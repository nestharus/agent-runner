//! Original-owner completed-turn custody and exact, non-executing settlement.
//! ## Declared roles
//! accessor, validator, orchestration, mapper
use super::*;
use crate::diagnostic_recorder::process_recorder;
use oulipoly_agent_messenger::ReturnedArtifactRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const COMPLETED_TURN_RECOVERY_IDENTITY_LIMIT: usize = 100;
const COMPLETED_TURN_RECOVERY_SQL: &str =
    "SELECT c.invocation_uuid,c.settlement_id,c.owner_json,c.effects_json,
            c.context_json,c.content_sha256,c.committed_at IS NOT NULL,
            c.tails_json,i.provider_name,i.provider_session_id,i.session_id
     FROM completed_turns c INDEXED BY completed_turns_recovery_pending
     JOIN invocations i ON i.id=c.invocation_id
     WHERE c.recovery_pending=1
     ORDER BY c.invocation_id";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletedTurnEffects {
    pub invocation_row_id: i64,
    pub delivery_ids: Vec<String>,
    pub session_id: String,
    pub turn_generation_id: String,
    pub submitted_evidence: Option<String>,
    pub confirmed_evidence: Option<String>,
    pub observed_at: i64,
    pub returned_artifacts: Vec<ReturnedArtifactRef>,
    pub resume_acceptance_status: Option<String>,
    pub resume_acceptance_evidence: Option<String>,
    pub success: bool,
    pub exit_code: i32,
    pub error_category: Option<String>,
    pub terminal_reason: Option<String>,
}
impl CompletedTurnEffects {
    pub fn input(&self) -> ProviderTurnEffectInput<'_> {
        ProviderTurnEffectInput {
            invocation_row_id: self.invocation_row_id,
            delivery_ids: &self.delivery_ids,
            accept_delivery_if_missing: true,
            session_id: &self.session_id,
            turn_generation_id: &self.turn_generation_id,
            submitted_evidence: self.submitted_evidence.as_deref(),
            confirmed_evidence: self.confirmed_evidence.as_deref(),
            observed_at: self.observed_at,
            returned_artifacts: &self.returned_artifacts,
            resume_acceptance_status: self.resume_acceptance_status.as_deref(),
            resume_acceptance_evidence: self.resume_acceptance_evidence.as_deref(),
            success: self.success,
            exit_code: self.exit_code,
            error_category: self.error_category.as_deref(),
            terminal_reason: self.terminal_reason.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedTurnRecord {
    pub invocation_uuid: String,
    pub settlement_id: String,
    pub effects: CompletedTurnEffects,
    pub context: serde_json::Value,
    pub committed: bool,
    pub tails: serde_json::Value,
}

/// A completed turn that still owns either settlement or post-settlement
/// recovery work.  `phase` deliberately distinguishes a turn whose State
/// effects have not committed from one whose claim-dependent tails have not
/// completed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletedTurnRecoveryIdentity {
    pub invocation_uuid: String,
    pub phase: String,
}

#[derive(Debug, Clone)]
struct CompletedTurnRecoveryDuty {
    invocation_uuid: String,
    settlement_id: String,
    provider_name: String,
    provider_session: String,
    original_wake_claim: Option<String>,
    phase: String,
}

/// The point at which a retained completed-turn conflict was observed.
/// Diagnostics must not erase effects that may already have happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletedTurnMigrationStage {
    BuiltInBeforeEffects,
    ExternalBeforeProvider,
    ExternalAfterProviderBeforeHostApply,
    JournalRecoveryBeforeEffects,
}

/// Retained-duty conflict granularity. Built-in migration knows the concrete
/// native session; an external provider is conservative for the whole account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletedTurnMigrationScope {
    BuiltInExact,
    ExternalProviderWide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MigrationLockMode {
    Shared,
    Exclusive,
}

/// Process-shared effect fence. This owns file locks only: no SQLite
/// transaction or connection is retained while provider, transcript, journal,
/// or filesystem work runs.
#[derive(Debug, Default)]
struct MigrationLockFiles(Vec<(std::fs::File, MigrationLockMode)>);

impl Drop for MigrationLockFiles {
    fn drop(&mut self) {
        for (file, _) in self.0.iter().rev() {
            let _ = <std::fs::File as fs4::FileExt>::unlock(file);
        }
    }
}

#[derive(Debug)]
pub struct CompletedTurnMigrationFence {
    _files: MigrationLockFiles,
    chain_id: String,
    target_provider: String,
    target_session: Option<String>,
    scope: CompletedTurnMigrationScope,
}

fn migration_lock_key(kind: &str, values: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    for value in values {
        digest.update([0]);
        digest.update(value.as_bytes());
    }
    format!("{kind}-{:x}.lock", digest.finalize())
}

fn acquire_migration_locks(
    state_path: &Path,
    mut scopes: Vec<(String, MigrationLockMode)>,
) -> Result<MigrationLockFiles, String> {
    scopes.sort();
    scopes.dedup();
    let parent = state_path
        .parent()
        .ok_or("completed_turn_state_identity_unavailable")?;
    let root = parent.join(".completed-turn-migration-locks");
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("completed_turn_migration_fence_unavailable: {error}"))?;
    let mut held = MigrationLockFiles(Vec::with_capacity(scopes.len()));
    for (name, mode) in scopes {
        let path = root.join(name);
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|error| format!("completed_turn_migration_fence_unavailable: {error}"))?;
        let result = match mode {
            MigrationLockMode::Shared => <std::fs::File as fs4::FileExt>::try_lock_shared(&file),
            MigrationLockMode::Exclusive => <std::fs::File as fs4::FileExt>::try_lock(&file),
        };
        result.map_err(|error| {
            format!("completed_turn_migration_in_progress: effect scope is already owned: {error}")
        })?;
        held.0.push((file, mode));
    }
    Ok(held)
}

fn completed_turn_tails_finished(tails: &serde_json::Value) -> bool {
    matches!(
        tails.get("native").and_then(serde_json::Value::as_str),
        Some("complete_or_standalone")
    ) && matches!(
        tails.get("delivery").and_then(serde_json::Value::as_str),
        Some("complete" | "not_applicable")
    ) && matches!(
        tails.get("idle").and_then(serde_json::Value::as_str),
        Some("complete" | "no_runtime")
    ) && matches!(
        tails.get("wake").and_then(serde_json::Value::as_str),
        Some("no_pending_at_recheck" | "distinct_next_turn_pending" | "no_mailbox")
    )
}
fn encode(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}
fn fingerprint(owner: &Option<String>, effects: &str, context: &str) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(encode(&(owner, effects, context))?.as_bytes())
    ))
}
fn custody_error(e: sqlite::Error) -> String {
    format!("completed turn custody: {e}")
}

#[derive(Serialize, Deserialize)]
struct CustodyOwner {
    state_path: PathBuf,
    volume: u64,
    file: u64,
    native: Option<ProviderLaunchOwnerFence>,
}
impl StateDb {
    fn completed_turn_recovery_duties(&self) -> Result<Vec<CompletedTurnRecoveryDuty>, String> {
        let mut stmt = self
            .conn
            .prepare(COMPLETED_TURN_RECOVERY_SQL)
            .map_err(custody_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })
            .map_err(custody_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(custody_error)?;
        rows.into_iter()
            .filter_map(
                |(
                    invocation_uuid,
                    settlement_id,
                    owner_json,
                    effects_json,
                    context_json,
                    content_sha256,
                    committed,
                    tails_json,
                    provider_name,
                    provider_session_id,
                    session_id,
                )| {
                    let current_fingerprint =
                        match fingerprint(&owner_json, &effects_json, &context_json) {
                            Ok(value) => value,
                            Err(error) => return Some(Err(error)),
                        };
                    if current_fingerprint != content_sha256 {
                        return Some(Err("completed_turn_corrupt_admission".into()));
                    }
                    let tails: serde_json::Value = match serde_json::from_str(&tails_json) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error.to_string())),
                    };
                    if committed && completed_turn_tails_finished(&tails) {
                        return None;
                    }
                    let context: serde_json::Value = match serde_json::from_str(&context_json) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error.to_string())),
                    };
                    let effects: CompletedTurnEffects = match serde_json::from_str(&effects_json) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error.to_string())),
                    };
                    let provider_session = provider_session_id
                        .or(session_id)
                        .or_else(|| {
                            context
                                .get("provider_session")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or(effects.session_id);
                    Some(Ok(CompletedTurnRecoveryDuty {
                        invocation_uuid,
                        settlement_id,
                        provider_name,
                        provider_session,
                        original_wake_claim: context
                            .get("original_wake_claim")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                        phase: if committed {
                            "committed_tail_incomplete"
                        } else {
                            "settlement_pending"
                        }
                        .to_string(),
                    }))
                },
            )
            .collect()
    }

    fn validate_resolved_resume_identity(&self, resolved: &ResolvedResume) -> Result<(), String> {
        let current: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_chain_segments WHERE chain_id=?1
                 AND provider_name=?2 AND session_id=?3 AND ended_at IS NULL)",
                sqlite::params![
                    resolved.chain_id,
                    resolved.active_provider,
                    resolved.active_session_id
                ],
                |row| row.get(0),
            )
            .map_err(custody_error)?;
        if current {
            Ok(())
        } else {
            Err("completed_turn_resume_identity_changed_or_unavailable".into())
        }
    }

    fn duty_is_on_chain(
        &self,
        duty: &CompletedTurnRecoveryDuty,
        chain_id: &str,
    ) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_chain_segments
                 WHERE chain_id=?1 AND provider_name=?2 AND session_id=?3)",
                params![chain_id, duty.provider_name, duty.provider_session],
                |row| row.get(0),
            )
            .map_err(custody_error)
    }

    fn migration_chain_memberships(
        &self,
        provider: &str,
        session: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT chain_id FROM session_chain_segments
                 WHERE provider_name=?1 AND session_id=?2 ORDER BY chain_id",
            )
            .map_err(custody_error)?;
        stmt.query_map(params![provider, session], |row| row.get(0))
            .map_err(custody_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(custody_error)
    }

    fn migration_scopes(
        &self,
        chain_id: &str,
        target_provider: &str,
        target_session: Option<&str>,
        scope: CompletedTurnMigrationScope,
    ) -> Result<Vec<(String, MigrationLockMode)>, String> {
        let mut scopes = vec![(
            migration_lock_key("chain", &[chain_id]),
            MigrationLockMode::Exclusive,
        )];
        scopes.push((
            migration_lock_key("provider", &[target_provider]),
            match scope {
                CompletedTurnMigrationScope::BuiltInExact => MigrationLockMode::Shared,
                CompletedTurnMigrationScope::ExternalProviderWide => MigrationLockMode::Exclusive,
            },
        ));
        if scope == CompletedTurnMigrationScope::BuiltInExact {
            let session = target_session.ok_or(
                "completed_turn_migration_scope_invalid: built-in migration requires a concrete session",
            )?;
            scopes.push((
                migration_lock_key("provider-session", &[target_provider, session]),
                MigrationLockMode::Exclusive,
            ));
        }
        Ok(scopes)
    }

    fn completed_turn_conflicts(
        &self,
        chain_id: &str,
        target_provider: &str,
        target_session: Option<&str>,
        scope: CompletedTurnMigrationScope,
    ) -> Result<Vec<String>, String> {
        let mut conflicts = Vec::new();
        for duty in self.completed_turn_recovery_duties()? {
            let target_conflict = duty.provider_name == target_provider
                && match scope {
                    CompletedTurnMigrationScope::ExternalProviderWide => true,
                    CompletedTurnMigrationScope::BuiltInExact => {
                        target_session.is_some_and(|session| duty.provider_session == session)
                    }
                };
            if target_conflict || self.duty_is_on_chain(&duty, chain_id)? {
                conflicts.push(format!("{}[{}]", duty.invocation_uuid, duty.phase));
            }
        }
        Ok(conflicts)
    }

    fn refuse_completed_turn_migration_target_stage(
        &self,
        fence: &CompletedTurnMigrationFence,
        target_session: Option<&str>,
        scope: CompletedTurnMigrationScope,
        stage: CompletedTurnMigrationStage,
    ) -> Result<(), String> {
        let conflicts = self.completed_turn_conflicts(
            &fence.chain_id,
            &fence.target_provider,
            target_session,
            scope,
        )?;
        if conflicts.is_empty() {
            return Ok(());
        }
        let consequence = match stage {
            CompletedTurnMigrationStage::BuiltInBeforeEffects => {
                "built-in transcript publication and segment rotation were not performed"
            }
            CompletedTurnMigrationStage::ExternalBeforeProvider => {
                "external provider execution, artifact publication, and host State apply were not performed"
            }
            CompletedTurnMigrationStage::ExternalAfterProviderBeforeHostApply => {
                "external provider execution or artifact publication may already have occurred; host State apply was not performed; the recovery journal remains authoritative"
            }
            CompletedTurnMigrationStage::JournalRecoveryBeforeEffects => {
                "journal recovery effects were not performed; the recovery journal remains authoritative"
            }
        };
        Err(format!(
            "completed_turn_target_pending: {}; target={}/{}; stage={stage:?}; {consequence}",
            conflicts.join(","),
            fence.target_provider,
            target_session.unwrap_or("<provider-wide>")
        ))
    }

    fn refuse_completed_turn_migration_stage(
        &self,
        fence: &CompletedTurnMigrationFence,
        stage: CompletedTurnMigrationStage,
    ) -> Result<(), String> {
        self.refuse_completed_turn_migration_target_stage(
            fence,
            fence.target_session.as_deref(),
            fence.scope,
            stage,
        )
    }

    /// Acquire process-shared protection before migration effects and check
    /// retained duties while that protection is held.
    pub fn begin_completed_turn_migration(
        &self,
        resolved: &ResolvedResume,
        target_provider: &str,
        target_session: Option<&str>,
        scope: CompletedTurnMigrationScope,
        stage: CompletedTurnMigrationStage,
    ) -> Result<CompletedTurnMigrationFence, String> {
        self.validate_resolved_resume_identity(resolved)?;
        let state_path = self
            .completion_authority_state_path()
            .ok_or("completed_turn_state_identity_unavailable")?;
        let files = acquire_migration_locks(
            state_path,
            self.migration_scopes(&resolved.chain_id, target_provider, target_session, scope)?,
        )?;
        // The original tuple can become stale while this caller waits for a
        // cooperating migration/admission fence. Revalidate after acquisition
        // so protection begins from the still-current immutable identity.
        self.validate_resolved_resume_identity(resolved)?;
        let fence = CompletedTurnMigrationFence {
            _files: files,
            chain_id: resolved.chain_id.clone(),
            target_provider: target_provider.to_string(),
            target_session: target_session.map(str::to_string),
            scope,
        };
        self.refuse_completed_turn_migration_stage(&fence, stage)?;
        Ok(fence)
    }

    /// Journal recovery can outlive the source segment that produced it, so it
    /// binds directly to the durable record's chain/target rather than claiming
    /// the caller's stale resolved tuple is still current.
    pub fn begin_completed_turn_journal_recovery(
        &self,
        chain_id: &str,
        target_provider: &str,
        target_session: Option<&str>,
    ) -> Result<CompletedTurnMigrationFence, String> {
        let state_path = self
            .completion_authority_state_path()
            .ok_or("completed_turn_state_identity_unavailable")?;
        let scope = if target_session.is_some() {
            CompletedTurnMigrationScope::BuiltInExact
        } else {
            CompletedTurnMigrationScope::ExternalProviderWide
        };
        let files = acquire_migration_locks(
            state_path,
            self.migration_scopes(chain_id, target_provider, target_session, scope)?,
        )?;
        let fence = CompletedTurnMigrationFence {
            _files: files,
            chain_id: chain_id.to_string(),
            target_provider: target_provider.to_string(),
            target_session: target_session.map(str::to_string),
            scope,
        };
        self.refuse_completed_turn_migration_stage(
            &fence,
            CompletedTurnMigrationStage::JournalRecoveryBeforeEffects,
        )?;
        Ok(fence)
    }

    /// Recheck a now-known external native target while retaining the original
    /// provider-wide effect fence. Same-account duties for another concrete
    /// native session remain unrelated once this identity is available.
    pub fn recheck_completed_turn_migration_exact_target(
        &self,
        fence: &CompletedTurnMigrationFence,
        target_session: &str,
        stage: CompletedTurnMigrationStage,
    ) -> Result<(), String> {
        self.refuse_completed_turn_migration_target_stage(
            fence,
            Some(target_session),
            CompletedTurnMigrationScope::BuiltInExact,
            stage,
        )
    }

    fn custody_owner(
        &self,
        native: Option<ProviderLaunchOwnerFence>,
    ) -> Result<CustodyOwner, String> {
        let path = self
            .completion_authority_state_path()
            .ok_or("completed_turn_state_identity_unavailable")?;
        let identity = self
            .completion_authority_state
            .as_ref()
            .ok_or("completed_turn_state_identity_unavailable")?;
        Ok(CustodyOwner {
            state_path: path.into(),
            volume: identity.file.volume,
            file: identity.file.file,
            native,
        })
    }
    fn validate_custody_owner(&self, encoded: Option<String>) -> Result<CustodyOwner, String> {
        let owner: CustodyOwner =
            serde_json::from_str(&encoded.ok_or("completed_turn_owner_missing")?)
                .map_err(|e| e.to_string())?;
        let current = self.custody_owner(None)?;
        if current.state_path != owner.state_path
            || current.volume != owner.volume
            || current.file != owner.file
        {
            return Err("completed_turn_original_state_identity_conflict".into());
        }
        Ok(owner)
    }
    /// Called by the actual executor with its caller-supplied row and mutation
    /// authority, before unlink. This is not the terminal artifact projection.
    pub fn retain_completed_turn_selection(
        &self,
        authority: InvocationMutationAuthority<'_>,
        row: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), String> {
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(custody_error)?;
        super::provider_launch_lifecycle::validate_invocation_mutation_authority(
            &tx, row, authority,
        )?;
        let identity = Self::load_invocation_identity_for_returned_artifacts(&tx, row)?;
        Self::validate_returned_artifact_refs(&identity, refs)?;
        let json = encode(&refs)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT references_json FROM completed_turn_selections WHERE invocation_id=?1",
                [row],
                |r| r.get(0),
            )
            .optional()
            .map_err(custody_error)?;
        match previous {
            Some(old) if old != json => return Err("completed_turn_selection_conflict".into()),
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO completed_turn_selections VALUES (?1,?2)",
                    params![row, json],
                )
                .map_err(custody_error)?;
            }
        }
        tx.commit().map_err(custody_error)
    }

    /// Immutable admission from the original owner's retained fence. No caller
    /// can create a native admission by supplying its persisted invocation UUID.
    pub fn admit_completed_turn(
        &self,
        authority: InvocationMutationAuthority<'_>,
        effects: &CompletedTurnEffects,
        context: &serde_json::Value,
    ) -> Result<String, String> {
        let row = effects.invocation_row_id;
        // Admission shares the provider-wide scope (so disjoint built-in
        // sessions remain concurrent), and exclusively owns its concrete
        // provider/session plus every historical chain carrying that identity.
        // These are process file locks only; the SQLite writer starts later.
        let admission_identity = self
            .conn
            .query_row(
                "SELECT provider_name,COALESCE(provider_session_id,session_id,?2)
                 FROM invocations WHERE id=?1",
                params![row, effects.session_id],
                |record| Ok((record.get::<_, String>(0)?, record.get::<_, String>(1)?)),
            )
            .map_err(custody_error)?;
        let admission_chains =
            self.migration_chain_memberships(&admission_identity.0, &admission_identity.1)?;
        let mut admission_scopes = vec![
            (
                migration_lock_key("provider", &[&admission_identity.0]),
                MigrationLockMode::Shared,
            ),
            (
                migration_lock_key(
                    "provider-session",
                    &[&admission_identity.0, &admission_identity.1],
                ),
                MigrationLockMode::Exclusive,
            ),
        ];
        admission_scopes.extend(admission_chains.iter().map(|chain| {
            (
                migration_lock_key("chain", &[chain]),
                MigrationLockMode::Exclusive,
            )
        }));
        let state_path = self
            .completion_authority_state_path()
            .ok_or("completed_turn_state_identity_unavailable")?;
        let _admission_fence = acquire_migration_locks(state_path, admission_scopes)?;
        #[cfg(test)]
        tests::WITH_COMPLETED_TURN_ADMISSION_FENCE.with_borrow_mut(|hook| {
            if let Some(hook) = hook.take() {
                hook();
            }
        });
        let current_identity = self
            .conn
            .query_row(
                "SELECT provider_name,COALESCE(provider_session_id,session_id,?2)
                 FROM invocations WHERE id=?1",
                params![row, effects.session_id],
                |record| Ok((record.get::<_, String>(0)?, record.get::<_, String>(1)?)),
            )
            .map_err(custody_error)?;
        let current_chains =
            self.migration_chain_memberships(&current_identity.0, &current_identity.1)?;
        if current_identity != admission_identity || current_chains != admission_chains {
            return Err("completed_turn_admission_scope_changed: retry without effects".into());
        }
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(custody_error)?;
        super::provider_launch_lifecycle::validate_invocation_mutation_authority(
            &tx, row, authority,
        )?;
        let invocation = Self::load_invocation_for_finalize(&tx, row)?;
        let native = match authority {
            InvocationMutationAuthority::Standalone => None,
            InvocationMutationAuthority::ProviderLaunch(f) => Some(f.clone()),
        };
        let owner = Some(encode(&self.custody_owner(native)?)?);
        let effects_json = encode(effects)?;
        let context_json = encode(context)?;
        let digest = fingerprint(&owner, &effects_json, &context_json)?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT settlement_id,content_sha256 FROM completed_turns WHERE invocation_id=?1",
                [row],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(custody_error)?;
        if let Some((id, old)) = previous {
            return if old == digest {
                Ok(id)
            } else {
                Err("completed_turn_admission_conflict".into())
            };
        }
        Self::validate_invocation_is_running(row, &invocation.status)?;
        let identity = Self::load_invocation_identity_for_returned_artifacts(&tx, row)?;
        Self::validate_returned_artifact_refs(&identity, &effects.returned_artifacts)?;
        let selected: Option<String> = tx
            .query_row(
                "SELECT references_json FROM completed_turn_selections WHERE invocation_id=?1",
                [row],
                |r| r.get(0),
            )
            .optional()
            .map_err(custody_error)?;
        if selected.is_some_and(|json| encode(&effects.returned_artifacts).as_ref() != Ok(&json)) {
            return Err("completed_turn_selection_conflict".into());
        }
        let id = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO completed_turns (invocation_id,invocation_uuid,settlement_id,owner_json,effects_json,context_json,content_sha256) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![row,invocation.invocation_uuid,id,owner,effects_json,context_json,digest]).map_err(custody_error)?;
        tx.commit().map_err(custody_error)?;
        Ok(id)
    }

    pub fn completed_turn(&self, uuid: &str) -> Result<Option<CompletedTurnRecord>, String> {
        let row = self.conn.query_row("SELECT settlement_id,owner_json,effects_json,context_json,content_sha256,committed_at IS NOT NULL,tails_json FROM completed_turns WHERE invocation_uuid=?1",[uuid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,bool>(5)?,r.get::<_,String>(6)?))).optional().map_err(custody_error)?;
        let Some((settlement_id, owner, effects, context, digest, committed, tails)) = row else {
            return Ok(None);
        };
        if fingerprint(&owner, &effects, &context)? != digest {
            return Err("completed_turn_corrupt_admission".into());
        }
        Ok(Some(CompletedTurnRecord {
            invocation_uuid: uuid.into(),
            settlement_id,
            effects: serde_json::from_str(&effects).map_err(|e| e.to_string())?,
            context: serde_json::from_str(&context).map_err(|e| e.to_string())?,
            committed,
            tails: serde_json::from_str(&tails).map_err(|e| e.to_string())?,
        }))
    }

    fn completed_turns_for_session_in(
        &self,
        session: &str,
        duties: &[CompletedTurnRecoveryDuty],
    ) -> Result<Vec<String>, String> {
        let mut pending = Vec::new();
        for duty in duties {
            let related_chain: bool = self
                .conn
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM session_chain_segments original
                         JOIN session_chain_segments target ON target.chain_id=original.chain_id
                         WHERE original.provider_name=?1 AND original.session_id=?2
                           AND target.session_id=?3 AND target.ended_at IS NULL)",
                    params![duty.provider_name, duty.provider_session, session],
                    |row| row.get(0),
                )
                .map_err(custody_error)?;
            if duty.provider_session == session || related_chain {
                pending.push(format!("{}[{}]", duty.invocation_uuid, duty.phase));
            }
        }
        Ok(pending)
    }

    pub fn completed_turns_for_session(&self, session: &str) -> Result<Vec<String>, String> {
        let duties = self.completed_turn_recovery_duties()?;
        self.completed_turns_for_session_in(session, &duties)
    }

    /// A resolver's chain choice is narrower than a native session string.
    /// Verify the current provider/chain/session tuple in State before using it
    /// to scope custody; a caller-supplied UUID alone is never authority.
    pub fn refuse_completed_turn_resolved_resume(
        &self,
        resolved: &ResolvedResume,
    ) -> Result<(), String> {
        self.validate_resolved_resume_identity(resolved)?;
        let mut pending = Vec::new();
        for duty in self.completed_turn_recovery_duties()? {
            if (duty.provider_name == resolved.active_provider
                && duty.provider_session == resolved.active_session_id)
                || self.duty_is_on_chain(&duty, &resolved.chain_id)?
            {
                pending.push(format!("{}[{}]", duty.invocation_uuid, duty.phase));
            }
        }
        Self::refuse_pending_completed_turns(pending)
    }

    /// Recheck the identity that migration will actually materialize.  A
    /// session-less target is used only when an external provider must be
    /// invoked to discover its native session; in that case the refusal is
    /// intentionally provider-wide and happens before provider execution.
    pub fn refuse_completed_turn_migration_target(
        &self,
        resolved: &ResolvedResume,
        target_provider: &str,
        target_session: Option<&str>,
    ) -> Result<(), String> {
        let duties = self.completed_turn_recovery_duties()?;
        if duties.is_empty() {
            return Ok(());
        }
        self.validate_resolved_resume_identity(resolved)?;
        let mut conflicts = Vec::new();
        for duty in duties {
            let target_conflict = duty.provider_name == target_provider
                && target_session.is_none_or(|session| duty.provider_session == session);
            if target_conflict || self.duty_is_on_chain(&duty, &resolved.chain_id)? {
                conflicts.push(format!("{}[{}]", duty.invocation_uuid, duty.phase));
            }
        }
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "completed_turn_target_pending: {}; target={}/{}; no transcript publication, segment rotation, external materialization, or provider execution",
                conflicts.join(","),
                target_provider,
                target_session.unwrap_or("<provider-wide-before-execution>")
            ))
        }
    }

    pub fn refuse_completed_turn_resume(&self, session: &str) -> Result<(), String> {
        let pending = self.completed_turns_for_session(session)?;
        Self::refuse_pending_completed_turns(pending)
    }

    fn refuse_pending_completed_turns(pending: Vec<String>) -> Result<(), String> {
        if pending.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "completed_turn_pending: {}; use completed-turn --invocation <uuid> --settle; no provider execution",
                pending.join(",")
            ))
        }
    }

    /// Serialize custody admission with destructive manual claim coordination.
    /// State is first; neither the sidecar namespace nor its SQLite writer may
    /// wait beneath it. Keep the reservation until the sidecar release commits.
    pub fn coordinate_manual_resume(
        &self,
        session: &str,
    ) -> Result<crate::mailbox::ManualWakeCoordination, String> {
        self.coordinate_manual_resume_target(session, None)
    }

    pub fn coordinate_resolved_manual_resume(
        &self,
        resolved: &ResolvedResume,
    ) -> Result<crate::mailbox::ManualWakeCoordination, String> {
        self.coordinate_manual_resume_target(&resolved.active_session_id, Some(resolved))
    }

    fn coordinate_manual_resume_target(
        &self,
        session: &str,
        resolved: Option<&ResolvedResume>,
    ) -> Result<crate::mailbox::ManualWakeCoordination, String> {
        // Recorder initialization and selection must precede the State writer
        // reservation; the nested sidecar open uses only deferred handoff.
        let recorder = process_recorder();
        #[cfg(test)]
        tests::BEFORE_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            if let Some(hook) = hook.take() {
                hook();
            }
        });
        let _reservation =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(custody_error)?;
        #[cfg(test)]
        tests::WITH_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            if let Some(hook) = hook.take() {
                hook();
            }
        });
        // This writer reservation is the manual State authority boundary.
        // Scan once and use the same snapshot for refusal and retained-claim
        // transport; do not perform a duplicate full retained-duty scan.
        let duties = self.completed_turn_recovery_duties()?;
        let pending = if let Some(resolved) = resolved {
            self.validate_resolved_resume_identity(resolved)?;
            let mut pending = Vec::new();
            for duty in &duties {
                if (duty.provider_name == resolved.active_provider
                    && duty.provider_session == resolved.active_session_id)
                    || self.duty_is_on_chain(duty, &resolved.chain_id)?
                {
                    pending.push(format!("{}[{}]", duty.invocation_uuid, duty.phase));
                }
            }
            pending
        } else {
            // Legacy callers have no authenticated chain distinction. Preserve
            // conservative session-scoped refusal rather than guessing one.
            self.completed_turns_for_session_in(session, &duties)?
        };
        Self::refuse_pending_completed_turns(pending)?;
        let retained_claims = duties
            .into_iter()
            .filter_map(|duty| {
                duty.original_wake_claim
                    .map(|claim_token| crate::mailbox::RetainedWakeClaim {
                        invocation_uuid: duty.invocation_uuid,
                        settlement_id: duty.settlement_id,
                        claim_token,
                        phase: duty.phase,
                    })
            })
            .collect::<Vec<_>>();
        let state_path = self
            .completion_authority_state_path()
            .ok_or("completed_turn_state_identity_unavailable")?;
        let path = crate::mailbox::MailboxDb::path_for_state_db(state_path);
        let authority = crate::mailbox::MailboxAuthorityFence::try_acquire(&path)
            .map_err(|e| format!("manual_resume_authority_unavailable: {e}"))?;
        let mut sidecar =
            crate::mailbox::MailboxDb::open_existing_for_completion_authority_deferred(
                &authority, &recorder,
            )?;
        sidecar.coordinate_manual_resume_without_wait(session, &retained_claims)
    }

    pub fn completed_turn_identities(&self) -> Result<Vec<CompletedTurnRecoveryIdentity>, String> {
        Ok(self
            .completed_turn_recovery_duties()?
            .into_iter()
            .take(COMPLETED_TURN_RECOVERY_IDENTITY_LIMIT)
            .map(|duty| CompletedTurnRecoveryIdentity {
                invocation_uuid: duty.invocation_uuid,
                phase: duty.phase,
            })
            .collect())
    }

    /// Settles only this admitted effect set. The retained fence never escapes
    /// as a general-purpose mutation scope. Validation is repeated under State.
    pub fn settle_completed_turn(&self, record: &CompletedTurnRecord) -> Result<(), String> {
        let current = self
            .completed_turn(&record.invocation_uuid)?
            .ok_or("completed_turn_missing")?;
        if current.settlement_id != record.settlement_id
            || current.effects != record.effects
            || current.context != record.context
        {
            return Err("completed_turn_recovery_conflict".into());
        }
        let owner: Option<String> = self
            .conn
            .query_row(
                "SELECT owner_json FROM completed_turns WHERE settlement_id=?1",
                [&record.settlement_id],
                |r| r.get(0),
            )
            .map_err(custody_error)?;
        let owner = self.validate_custody_owner(owner)?.native;
        let authority = owner
            .as_ref()
            .map(InvocationMutationAuthority::ProviderLaunch)
            .unwrap_or(InvocationMutationAuthority::Standalone);
        self.apply_completed_turn_effects(authority, record.effects.input(), &record.settlement_id)
    }

    pub fn record_completed_turn_tails(
        &self,
        uuid: &str,
        settlement: &str,
        tails: &serde_json::Value,
    ) -> Result<(), String> {
        let recovery_pending = i64::from(!completed_turn_tails_finished(tails));
        let changed = self.conn.execute("UPDATE completed_turns SET tails_json=?3,recovery_pending=?4 WHERE invocation_uuid=?1 AND settlement_id=?2 AND committed_at IS NOT NULL",params![uuid,settlement,encode(tails)?,recovery_pending]).map_err(custody_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err("completed_turn_not_committed".into())
        }
    }
}

pub(super) fn refuse_pending_turn(conn: &sqlite::Connection, row: i64) -> Result<(), String> {
    let pending: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM completed_turns WHERE invocation_id=?1 AND committed_at IS NULL)",[row],|r|r.get(0)).map_err(custody_error)?;
    if pending {
        Err("process_integrity: completed_turn_pending; use completed-turn --invocation <uuid> --settle".into())
    } else {
        Ok(())
    }
}

pub(super) fn validate_settlement(
    conn: &sqlite::Connection,
    row: i64,
    id: &str,
    authority: InvocationMutationAuthority<'_>,
) -> Result<bool, String> {
    super::provider_launch_lifecycle::validate_mutation_authority(conn, row, authority)?;
    let (stored,committed): (String,bool) = conn.query_row("SELECT settlement_id,committed_at IS NOT NULL FROM completed_turns WHERE invocation_id=?1",[row],|r|Ok((r.get(0)?,r.get(1)?))).map_err(custody_error)?;
    if stored != id {
        return Err("completed_turn_settlement_conflict".into());
    }
    if let InvocationMutationAuthority::ProviderLaunch(owner) = authority {
        let cancelled: bool = conn.query_row("SELECT cancel_requested_at IS NOT NULL OR status IN ('cancelled','recovery_blocked') FROM provider_logical_launches WHERE logical_launch_id=?1",[owner.logical_launch_id.to_string()],|r|r.get(0)).map_err(custody_error)?;
        if cancelled {
            return Err(
                "completed_turn_authority_conflict: cancellation/recovery disposition".into(),
            );
        }
    }
    Ok(committed)
}

impl StateDb {
    pub fn complete_completed_turn_native(
        &self,
        uuid: &str,
        settlement: &str,
    ) -> Result<(), String> {
        let owner: Option<String> = self.conn.query_row("SELECT owner_json FROM completed_turns WHERE invocation_uuid=?1 AND settlement_id=?2 AND committed_at IS NOT NULL", params![uuid,settlement], |r|r.get(0)).map_err(custody_error)?;
        if let Some(owner) = self.validate_custody_owner(owner)?.native {
            self.complete_native_launch(&owner)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_recorder::{
        DiagnosticPhase, FlightRecorder, FlightRecorderReader, RecorderConfig,
        SqliteMeasurementGap, SqliteTransactionPhase, with_test_process_recorder,
    };
    use crate::sqlite_observability::{SqliteObservationPolicy, with_test_process_policy};
    use std::sync::mpsc;
    use std::time::Duration;

    thread_local! {
        pub(super) static BEFORE_MANUAL_RESERVATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
        pub(super) static WITH_MANUAL_RESERVATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
        pub(super) static WITH_COMPLETED_TURN_ADMISSION_FENCE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
    }

    fn manual_fixture(
        state: &StateDb,
        effects: &CompletedTurnEffects,
        uuid: &str,
    ) -> crate::mailbox::MailboxDb {
        state
            .bind_invocation_provider_session_start(
                InvocationMutationAuthority::Standalone,
                effects.invocation_row_id,
                &crate::ProviderSessionBinding {
                    provider_session_id: "session".into(),
                    capture_method: "fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        let mailbox = crate::mailbox::MailboxDb::open(
            &crate::mailbox::MailboxDb::path_for_state_db(state.path()),
        )
        .unwrap();
        mailbox.connection().execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,wake_invocation_uuid,wake_pid,reason,auto_wake_count) VALUES ('session','old','2000-01-01T00:00:00Z',?1,?2,'fixture',1)",params![uuid,i64::MAX]).unwrap();
        mailbox
    }

    fn resolved_manual_fixture(state: &StateDb) -> ResolvedResume {
        state
            .mint_imported_chain_if_absent("fixture", "session", &chrono::Utc::now(), "fixture")
            .unwrap();
        let chain = state
            .chain_id_for_segment("fixture", "session")
            .unwrap()
            .unwrap();
        let segment = state
            .active_chain_segment_snapshot(&chain)
            .unwrap()
            .unwrap();
        ResolvedResume {
            chain_id: segment.chain_id,
            active_provider: segment.active_provider,
            active_session_id: segment.active_session_id,
            model: None,
            model_name: None,
        }
    }

    #[test]
    fn resolved_manual_coordination_rejects_unverified_or_stale_identity() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let resolved = resolved_manual_fixture(&state);
        state
            .refuse_completed_turn_resolved_resume(&resolved)
            .unwrap();
        for field in ["chain", "provider", "session"] {
            let mut wrong = resolved.clone();
            match field {
                "chain" => wrong.chain_id = Uuid::new_v4().to_string(),
                "provider" => wrong.active_provider = "different".into(),
                _ => wrong.active_session_id = "different".into(),
            }
            assert!(
                state
                    .coordinate_resolved_manual_resume(&wrong)
                    .unwrap_err()
                    .contains("identity_changed_or_unavailable")
            );
            assert_eq!(
                mailbox
                    .wake_session_reader()
                    .wake_claim("session")
                    .unwrap()
                    .unwrap()
                    .claim_token,
                "old"
            );
        }
        state
            .rotate_chain_segment_transactionally(ChainSegmentRotationInput {
                chain_id: &resolved.chain_id,
                source_provider_name: "fixture",
                source_session_id: "session",
                target_provider_name: "successor",
                target_session_id: "next",
                changed_at: &chrono::Utc::now(),
                reason: oulipoly_core::TransitionReason::Manual,
            })
            .unwrap();
        assert!(
            state
                .coordinate_resolved_manual_resume(&resolved)
                .unwrap_err()
                .contains("identity_changed_or_unavailable")
        );
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn resolved_manual_coordination_rechecks_racing_custody_under_reservation() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let resolved = resolved_manual_fixture(&state);
        state
            .refuse_completed_turn_resolved_resume(&resolved)
            .unwrap();
        let competing = StateDb::open(state.path()).unwrap();
        BEFORE_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                admit(&competing, &effects);
            }));
        });
        assert!(
            state
                .coordinate_resolved_manual_resume(&resolved)
                .unwrap_err()
                .contains("completed_turn_pending")
        );
        assert_eq!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .unwrap()
                .claim_token,
            "old"
        );
    }

    #[test]
    fn manual_admission_rechecks_custody_committed_after_preflight() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        assert!(
            state
                .completed_turns_for_session("session")
                .unwrap()
                .is_empty()
        );
        let competing = StateDb::open(state.path()).unwrap();
        BEFORE_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                admit(&competing, &effects);
            }))
        });
        let error = state.coordinate_manual_resume("session").unwrap_err();
        assert!(error.contains("completed_turn_pending"), "{error}");
        assert_eq!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .unwrap()
                .claim_token,
            "old"
        );
        println!(
            "preflight-empty -> competing actual admission commit -> coordinated refusal; original claim retained"
        );
    }

    #[test]
    fn manual_coordination_serializes_admission_until_claim_commit() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let competing = StateDb::open(state.path()).unwrap();
        competing
            .conn
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        WITH_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                let error = competing
                    .admit_completed_turn(
                        InvocationMutationAuthority::Standalone,
                        &effects,
                        &serde_json::json!({"fixture":"State-only"}),
                    )
                    .unwrap_err();
                assert!(error.contains("locked"), "{error}");
            }))
        });
        assert_eq!(
            state.coordinate_manual_resume("session").unwrap(),
            crate::mailbox::ManualWakeCoordination::Released
        );
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        println!(
            "actual competing State admission could not commit inside coordinated release reservation"
        );
    }

    #[test]
    fn manual_coordination_sidecar_open_never_waits_for_blocked_recorder() {
        let (directory, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let recorder_root = directory.path().join("recorder");
        let recorder = FlightRecorder::open(
            &recorder_root,
            RecorderConfig {
                deferred_queue_capacity: 2,
                ..RecorderConfig::default()
            },
        )
        .unwrap();
        let release_writer = recorder.block_writer_for_test().unwrap();
        let worker_recorder = recorder.clone();
        let (completed, completion) = mpsc::channel();

        let worker = std::thread::spawn(move || {
            let result = with_test_process_recorder(worker_recorder, || {
                with_test_process_policy(SqliteObservationPolicy::all(), || {
                    state.coordinate_manual_resume("session")
                })
            });
            completed.send(result).unwrap();
        });

        let result = completion.recv_timeout(Duration::from_millis(250));
        release_writer.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            result.is_ok(),
            "manual-resume coordination waited for recorder progress while holding State"
        );
        assert_eq!(
            result.unwrap().unwrap(),
            crate::mailbox::ManualWakeCoordination::Released
        );
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        recorder.drain_deferred_for_test().unwrap();

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let observation = report
            .events
            .iter()
            .find(|record| {
                record.event.sqlite.as_ref().is_some_and(|sqlite| {
                    sqlite.query_family == "pid_mailbox.connection.open_completion_authority"
                })
            })
            .expect("manual-resume coordination retained sidecar-open evidence");
        assert_eq!(observation.event.phase, DiagnosticPhase::Released);
        assert_eq!(observation.event.parent_span_id, None);
        let evidence = observation.event.observation.sqlite.as_ref().unwrap();
        assert_eq!(
            evidence.transaction_phase,
            Some(SqliteTransactionPhase::ConnectionOpen)
        );
        assert!(evidence.execution_micros.is_some());
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::WriterAuthorityNotApplicable)
        );
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::CommitNotApplicable)
        );
    }

    #[test]
    fn committed_tail_remains_discoverable_and_protects_exact_original_claim() {
        let (_dir, state, uuid, effects) = fixture();
        let mut mailbox = manual_fixture(&state, &effects, &uuid);
        let original = resolved_manual_fixture(&state);
        let context = serde_json::json!({
            "provider_session": "session",
            "original_wake_claim": "old"
        });
        let settlement = state
            .admit_completed_turn(InvocationMutationAuthority::Standalone, &effects, &context)
            .unwrap();
        assert_eq!(
            state.completed_turn_identities().unwrap(),
            vec![CompletedTurnRecoveryIdentity {
                invocation_uuid: uuid.clone(),
                phase: "settlement_pending".into(),
            }]
        );
        let record = state.completed_turn(&uuid).unwrap().unwrap();
        state.settle_completed_turn(&record).unwrap();
        assert_eq!(
            state.completed_turn_identities().unwrap(),
            vec![CompletedTurnRecoveryIdentity {
                invocation_uuid: uuid.clone(),
                phase: "committed_tail_incomplete".into(),
            }]
        );

        state
            .mint_imported_chain_if_absent("distinct", "session", &chrono::Utc::now(), "fixture")
            .unwrap();
        let distinct_chain = state
            .chain_id_for_segment("distinct", "session")
            .unwrap()
            .unwrap();
        assert_ne!(distinct_chain, original.chain_id);
        let distinct = ResolvedResume {
            chain_id: distinct_chain,
            active_provider: "distinct".into(),
            active_session_id: "session".into(),
            model: None,
            model_name: None,
        };
        let error = state
            .coordinate_resolved_manual_resume(&distinct)
            .unwrap_err();
        assert!(error.contains("completed_turn_claim_pending"), "{error}");
        assert!(error.contains("committed_tail_incomplete"), "{error}");
        let claim = mailbox
            .wake_session_reader()
            .wake_claim("session")
            .unwrap()
            .unwrap();
        assert_eq!(claim.claim_token, "old");
        assert_eq!(claim.wake_invocation_uuid.as_deref(), Some(uuid.as_str()));

        mailbox
            .finish_completed_turn_bookkeeping("session", &uuid, &settlement, Some("old"), 0)
            .unwrap();
        state
            .record_completed_turn_tails(
                &uuid,
                &settlement,
                &serde_json::json!({
                    "native":"complete_or_standalone",
                    "delivery":"not_applicable",
                    "idle":"complete",
                    "wake":"no_pending_at_recheck"
                }),
            )
            .unwrap();
        assert!(state.completed_turn_identities().unwrap().is_empty());
        assert_eq!(
            state.coordinate_resolved_manual_resume(&distinct).unwrap(),
            crate::mailbox::ManualWakeCoordination::Absent
        );
    }

    #[test]
    fn manual_sidecar_contention_releases_state_and_preserves_claim() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let holder = sqlite::Connection::open(mailbox.path()).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let competitor = sqlite::Connection::open(state.path()).unwrap();
        competitor.busy_timeout(std::time::Duration::ZERO).unwrap();
        WITH_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                assert!(competitor.execute_batch("BEGIN IMMEDIATE").is_err());
            }))
        });
        let before = std::time::Instant::now();
        assert!(state.coordinate_manual_resume("session").is_err());
        assert!(
            before.elapsed() < std::time::Duration::from_secs(1),
            "sidecar ordinary wait spent under State"
        );
        let probe = sqlite::Connection::open(state.path()).unwrap();
        probe.busy_timeout(std::time::Duration::ZERO).unwrap();
        probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
        assert_eq!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .unwrap()
                .claim_token,
            "old"
        );
        holder.execute_batch("ROLLBACK").unwrap();
        println!("known-held State released while sidecar writer remained held; claim unchanged");
    }

    #[test]
    fn manual_namespace_contention_does_not_wait_under_state() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let path = mailbox.path().to_path_buf();
        drop(mailbox);
        let holder = crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&path).unwrap();
        let competitor = sqlite::Connection::open(state.path()).unwrap();
        competitor.busy_timeout(std::time::Duration::ZERO).unwrap();
        WITH_MANUAL_RESERVATION.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                assert!(competitor.execute_batch("BEGIN IMMEDIATE").is_err());
            }))
        });
        let before = std::time::Instant::now();
        let error = state.coordinate_manual_resume("session").unwrap_err();
        assert!(
            error.contains("manual_resume_authority_unavailable"),
            "{error}"
        );
        assert!(
            before.elapsed() < std::time::Duration::from_millis(400),
            "namespace wait spent under State"
        );
        let probe = sqlite::Connection::open(state.path()).unwrap();
        probe.busy_timeout(std::time::Duration::ZERO).unwrap();
        probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
        drop(holder);
        let mailbox = crate::mailbox::MailboxDb::open(&path).unwrap();
        assert_eq!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .unwrap()
                .claim_token,
            "old"
        );
        println!(
            "known-held State released while sidecar namespace remained held; claim unchanged"
        );
    }

    #[test]
    fn missing_or_unreadable_custody_is_not_empty_admission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        assert!(StateDb::open_existing(&path).is_err());
        assert!(!path.exists());
        let state = StateDb::open(&path).unwrap();
        assert!(state.coordinate_manual_resume("session").is_err());
        assert!(!crate::mailbox::MailboxDb::path_for_state_db(&path).exists());
        state
            .conn
            .execute_batch("ALTER TABLE completed_turns RENAME TO unavailable_completed_turns")
            .unwrap();
        assert!(state.coordinate_manual_resume("session").is_err());
    }

    #[test]
    fn completed_turn_recovery_plan_uses_only_the_pending_projection() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDb::open(&directory.path().join("state.db")).unwrap();
        let query = format!("EXPLAIN QUERY PLAN {COMPLETED_TURN_RECOVERY_SQL}");
        let details = state
            .conn
            .prepare(&query)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("completed_turns_recovery_pending")),
            "completed-turn recovery did not use its live projection: {details:?}"
        );
        assert!(
            details.iter().all(|detail| {
                !detail.contains("SCAN c")
                    || detail.contains("USING INDEX completed_turns_recovery_pending")
            }),
            "completed-turn recovery scanned terminal history: {details:?}"
        );
        assert!(state.completed_turn_identities().unwrap().is_empty());
    }

    // Independent intent: completed-turn-retention-decisions.md. This fixture
    // tests State custody/atomicity, not provider attestation or body capture.
    fn fixture() -> (tempfile::TempDir, StateDb, String, CompletedTurnEffects) {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let uuid = Uuid::new_v4().to_string();
        let id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.clone(),
                model_name: "model".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let refs = ["first", "second"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| ReturnedArtifactRef {
                version_id: format!("store://return/{uuid}/{name}/1"),
                name: name.into(),
                store_address: oulipoly_agent_messenger::StoreAddress {
                    workflow_run_id: format!("return:{uuid}"),
                    artifact_name: name.into(),
                    version: 1,
                },
                sha256: if i == 0 { "a" } else { "b" }.repeat(64),
                content_len: 3,
                format_hint: Some("application/octet-stream".into()),
                verdict_line: None,
                source: oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes,
                producer_invocation_uuid: uuid.parse().unwrap(),
                returned_at: chrono::Utc::now(),
            })
            .collect();
        let effects = CompletedTurnEffects {
            invocation_row_id: id,
            delivery_ids: vec!["incoming".into()],
            session_id: "session".into(),
            turn_generation_id: uuid.clone(),
            submitted_evidence: Some("submitted-proof".into()),
            confirmed_evidence: Some("confirmed-proof".into()),
            observed_at: 123,
            returned_artifacts: refs,
            resume_acceptance_status: Some("accepted".into()),
            resume_acceptance_evidence: Some("acceptance-proof".into()),
            success: true,
            exit_code: 0,
            error_category: None,
            terminal_reason: Some("completed".into()),
        };
        (dir, state, uuid, effects)
    }
    fn admit(state: &StateDb, effects: &CompletedTurnEffects) -> String {
        state
            .admit_completed_turn(
                InvocationMutationAuthority::Standalone,
                effects,
                &serde_json::json!({"fixture":"State-only"}),
            )
            .unwrap()
    }
    fn count(state: &StateDb, table: &str) -> i64 {
        state
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn bind_for_migration(state: &StateDb, effects: &CompletedTurnEffects) -> ResolvedResume {
        state
            .bind_invocation_provider_session_start(
                InvocationMutationAuthority::Standalone,
                effects.invocation_row_id,
                &crate::ProviderSessionBinding {
                    provider_session_id: effects.session_id.clone(),
                    capture_method: "fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        state
            .mint_imported_chain_if_absent(
                "fixture",
                &effects.session_id,
                &chrono::Utc::now(),
                "fixture",
            )
            .unwrap();
        let chain_id = state
            .chain_id_for_segment("fixture", &effects.session_id)
            .unwrap()
            .unwrap();
        ResolvedResume {
            chain_id,
            active_provider: "fixture".into(),
            active_session_id: effects.session_id.clone(),
            model: None,
            model_name: None,
        }
    }

    #[test]
    fn admission_fence_prevents_built_in_effects_then_becomes_durable_refusal() {
        let (_dir, state, _uuid, effects) = fixture();
        let resolved = bind_for_migration(&state, &effects);
        let contender = StateDb::open(state.path()).unwrap();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            WITH_COMPLETED_TURN_ADMISSION_FENCE.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    held_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }));
            });
            contender
                .admit_completed_turn(
                    InvocationMutationAuthority::Standalone,
                    &effects,
                    &serde_json::json!({"fixture":"concurrent-admission"}),
                )
                .unwrap()
        });
        held_rx.recv().unwrap();
        let error = state
            .begin_completed_turn_migration(
                &resolved,
                "fixture",
                Some("session"),
                CompletedTurnMigrationScope::BuiltInExact,
                CompletedTurnMigrationStage::BuiltInBeforeEffects,
            )
            .unwrap_err();
        assert!(
            error.contains("completed_turn_migration_in_progress"),
            "{error}"
        );
        // No SQLite writer is retained by the migration attempt.
        state
            .conn
            .execute_batch("BEGIN IMMEDIATE; ROLLBACK")
            .unwrap();
        release_tx.send(()).unwrap();
        let _settlement = thread.join().unwrap();
        let error = state
            .begin_completed_turn_migration(
                &resolved,
                "fixture",
                Some("session"),
                CompletedTurnMigrationScope::BuiltInExact,
                CompletedTurnMigrationStage::BuiltInBeforeEffects,
            )
            .unwrap_err();
        assert!(error.contains("completed_turn_target_pending"), "{error}");
        assert!(error.contains("built-in transcript publication"), "{error}");
    }

    #[test]
    fn migration_fences_block_conflicts_but_allow_disjoint_exact_admission() {
        let (_dir, state, _uuid, effects) = fixture();
        let resolved = bind_for_migration(&state, &effects);
        let built_in = state
            .begin_completed_turn_migration(
                &resolved,
                "fixture",
                Some("session"),
                CompletedTurnMigrationScope::BuiltInExact,
                CompletedTurnMigrationStage::BuiltInBeforeEffects,
            )
            .unwrap();
        let exact = StateDb::open(state.path()).unwrap();
        let error = exact
            .admit_completed_turn(
                InvocationMutationAuthority::Standalone,
                &effects,
                &serde_json::json!({"fixture":"same-exact"}),
            )
            .unwrap_err();
        assert!(
            error.contains("completed_turn_migration_in_progress"),
            "{error}"
        );

        let other_uuid = Uuid::new_v4().to_string();
        let other_row = state
            .start_invocation(&InvocationStart {
                invocation_uuid: other_uuid.clone(),
                model_name: "model".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let other_effects = CompletedTurnEffects {
            invocation_row_id: other_row,
            delivery_ids: vec![],
            session_id: "other-session".into(),
            turn_generation_id: other_uuid,
            submitted_evidence: None,
            confirmed_evidence: None,
            observed_at: 0,
            returned_artifacts: vec![],
            resume_acceptance_status: None,
            resume_acceptance_evidence: None,
            success: true,
            exit_code: 0,
            error_category: None,
            terminal_reason: Some("completed".into()),
        };
        let other_resolved = bind_for_migration(&state, &other_effects);
        assert_ne!(resolved.chain_id, other_resolved.chain_id);
        state
            .admit_completed_turn(
                InvocationMutationAuthority::Standalone,
                &other_effects,
                &serde_json::json!({"fixture":"disjoint-exact"}),
            )
            .unwrap();
        drop(built_in);
    }

    #[test]
    fn external_provider_wide_fence_blocks_disjoint_native_session_admission() {
        let (_dir, state, _uuid, effects) = fixture();
        let resolved = bind_for_migration(&state, &effects);
        let external = state
            .begin_completed_turn_migration(
                &resolved,
                "fixture",
                None,
                CompletedTurnMigrationScope::ExternalProviderWide,
                CompletedTurnMigrationStage::ExternalBeforeProvider,
            )
            .unwrap();
        let third_uuid = Uuid::new_v4().to_string();
        let third_row = state
            .start_invocation(&InvocationStart {
                invocation_uuid: third_uuid.clone(),
                model_name: "model".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let provider_wide_effects = CompletedTurnEffects {
            invocation_row_id: third_row,
            delivery_ids: vec![],
            session_id: "provider-wide-other-session".into(),
            turn_generation_id: third_uuid,
            submitted_evidence: None,
            confirmed_evidence: None,
            observed_at: 0,
            returned_artifacts: vec![],
            resume_acceptance_status: None,
            resume_acceptance_evidence: None,
            success: true,
            exit_code: 0,
            error_category: None,
            terminal_reason: Some("completed".into()),
        };
        state
            .bind_invocation_provider_session_start(
                InvocationMutationAuthority::Standalone,
                third_row,
                &crate::ProviderSessionBinding {
                    provider_session_id: provider_wide_effects.session_id.clone(),
                    capture_method: "fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();
        let error = state
            .admit_completed_turn(
                InvocationMutationAuthority::Standalone,
                &provider_wide_effects,
                &serde_json::json!({"fixture":"provider-wide-conflict"}),
            )
            .unwrap_err();
        assert!(
            error.contains("completed_turn_migration_in_progress"),
            "{error}"
        );
        drop(external);
    }

    #[test]
    fn late_external_conflict_reports_post_provider_stage_without_erasing_recovery() {
        let (_dir, state, _uuid, effects) = fixture();
        let resolved = bind_for_migration(&state, &effects);
        let fence = state
            .begin_completed_turn_migration(
                &resolved,
                "external-target",
                None,
                CompletedTurnMigrationScope::ExternalProviderWide,
                CompletedTurnMigrationStage::ExternalBeforeProvider,
            )
            .unwrap();
        // Model a non-cooperating/manual State writer discovered after provider
        // artifact+journal publication. This deliberately bypasses admission's
        // cooperative fence so the late-stage oracle remains discriminatory.
        let context = serde_json::json!({"provider_session":"external-session"});
        let effects_json = encode(&effects).unwrap();
        let context_json = encode(&context).unwrap();
        let owner: Option<String> = None;
        let digest = fingerprint(&owner, &effects_json, &context_json).unwrap();
        state.conn.execute(
            "INSERT INTO completed_turns
             (invocation_id,invocation_uuid,settlement_id,owner_json,effects_json,context_json,content_sha256)
             VALUES (?1,?2,?3,NULL,?4,?5,?6)",
            params![
                effects.invocation_row_id,
                effects.turn_generation_id,
                Uuid::new_v4().to_string(),
                effects_json,
                context_json,
                digest
            ],
        ).unwrap();
        state.conn.execute(
            "UPDATE invocations SET provider_name='external-target',provider_session_id='external-disjoint-session'
             WHERE id=?1",
            [effects.invocation_row_id],
        ).unwrap();
        state
            .recheck_completed_turn_migration_exact_target(
                &fence,
                "external-session",
                CompletedTurnMigrationStage::ExternalAfterProviderBeforeHostApply,
            )
            .unwrap();
        state
            .conn
            .execute(
                "UPDATE invocations SET provider_session_id='external-session' WHERE id=?1",
                [effects.invocation_row_id],
            )
            .unwrap();
        let error = state
            .recheck_completed_turn_migration_exact_target(
                &fence,
                "external-session",
                CompletedTurnMigrationStage::ExternalAfterProviderBeforeHostApply,
            )
            .unwrap_err();
        assert!(error.contains("may already have occurred"), "{error}");
        assert!(
            error.contains("host State apply was not performed"),
            "{error}"
        );
        assert!(
            error.contains("recovery journal remains authoritative"),
            "{error}"
        );
        assert_eq!(state.completed_turn_identities().unwrap().len(), 1);
    }

    #[test]
    fn completed_admission_is_not_success_and_restart_settles_once() {
        let (dir, state, uuid, effects) = fixture();
        state
            .retain_completed_turn_selection(
                InvocationMutationAuthority::Standalone,
                effects.invocation_row_id,
                &effects.returned_artifacts,
            )
            .unwrap();
        let id = admit(&state, &effects);
        assert_eq!(id, admit(&state, &effects));
        assert_eq!(count(&state, "completed_turns"), 1);
        assert_eq!(count(&state, "session_delivery_acknowledgements"), 0);
        assert_eq!(count(&state, "invocation_returned_artifacts"), 0);
        assert_eq!(
            state.get_invocation_by_uuid(&uuid).unwrap().unwrap().status,
            InvocationStatus::Running
        );
        assert!(
            state
                .finalize_invocation(
                    InvocationMutationAuthority::Standalone,
                    effects.invocation_row_id,
                    false,
                    1,
                    None,
                    None
                )
                .unwrap_err()
                .contains("completed_turn_pending")
        );
        drop(state);
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let record = state.completed_turn(&uuid).unwrap().unwrap();
        assert_eq!(record.settlement_id, id);
        state.settle_completed_turn(&record).unwrap();
        state.settle_completed_turn(&record).unwrap();
        assert!(state.completed_turn(&uuid).unwrap().unwrap().committed);
        let row = state.get_invocation_by_uuid(&uuid).unwrap().unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded);
        assert_eq!(row.resume_acceptance_status.as_deref(), Some("accepted"));
        assert_eq!(
            row.resume_acceptance_evidence.as_deref(),
            Some("acceptance-proof")
        );
        assert_eq!(count(&state, "session_delivery_acknowledgements"), 1);
        assert_eq!(count(&state, "invocation_returned_artifacts"), 2);
        assert_eq!(state.conn.query_row("SELECT invocation_count FROM providers WHERE model_name='model' AND provider_name='fixture'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        let acknowledgement:(i64,String,i64,String,i64) = state.conn.query_row("SELECT submitted_at,submitted_evidence,confirmed_at,confirmed_evidence,accepted_at FROM session_delivery_acknowledgements WHERE delivery_id='incoming'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(
            acknowledgement,
            (
                123,
                "submitted-proof".into(),
                123,
                "confirmed-proof".into(),
                123
            )
        );
    }
    #[test]
    fn completed_settlement_fault_rolls_back_all_effects_but_retains_duty() {
        let (_dir, state, uuid, effects) = fixture();
        admit(&state, &effects);
        let record = state.completed_turn(&uuid).unwrap().unwrap();
        state.conn.execute_batch("CREATE TRIGGER fault BEFORE INSERT ON providers BEGIN SELECT RAISE(ABORT,'retained-fault'); END").unwrap();
        for _ in 0..2 {
            assert!(
                state
                    .settle_completed_turn(&record)
                    .unwrap_err()
                    .contains("retained-fault")
            );
            assert!(!state.completed_turn(&uuid).unwrap().unwrap().committed);
            assert_eq!(count(&state, "session_delivery_acknowledgements"), 0);
            assert_eq!(count(&state, "invocation_returned_artifacts"), 0);
            let row = state.get_invocation_by_uuid(&uuid).unwrap().unwrap();
            assert_eq!(row.status, InvocationStatus::Running);
            assert!(row.resume_acceptance_status.is_none());
            assert!(row.resume_acceptance_evidence.is_none());
            assert_eq!(count(&state, "completed_turns"), 1);
        }
        state.conn.execute_batch("DROP TRIGGER fault").unwrap();
        state.settle_completed_turn(&record).unwrap();
    }
    #[test]
    fn completed_corruption_and_changed_selection_are_not_empty_success() {
        let (_dir, state, uuid, effects) = fixture();
        state
            .retain_completed_turn_selection(
                InvocationMutationAuthority::Standalone,
                effects.invocation_row_id,
                &effects.returned_artifacts,
            )
            .unwrap();
        let mut reversed = effects.clone();
        reversed.returned_artifacts.reverse();
        assert!(
            state
                .admit_completed_turn(
                    InvocationMutationAuthority::Standalone,
                    &reversed,
                    &serde_json::json!({})
                )
                .unwrap_err()
                .contains("selection_conflict")
        );
        assert!(state.completed_turn(&uuid).unwrap().is_none());
        admit(&state, &effects);
        state
            .conn
            .execute("UPDATE completed_turns SET context_json='{}'", [])
            .unwrap();
        assert!(
            state
                .completed_turn(&uuid)
                .unwrap_err()
                .contains("corrupt_admission")
        );
    }
    #[test]
    fn completed_copied_state_cannot_manufacture_original_owner() {
        let (dir, state, uuid, effects) = fixture();
        admit(&state, &effects);
        drop(state);
        let original = dir.path().join("state.db");
        let copy = dir.path().join("copy.db");
        std::fs::copy(&original, &copy).unwrap();
        let copied = StateDb::open(&copy).unwrap();
        let record = copied.completed_turn(&uuid).unwrap().unwrap();
        assert!(
            copied
                .settle_completed_turn(&record)
                .unwrap_err()
                .contains("original_state_identity_conflict")
        );
    }
}
