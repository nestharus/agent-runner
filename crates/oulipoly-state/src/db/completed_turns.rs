//! Original-owner completed-turn custody and exact, non-executing settlement.
//! ## Declared roles
//! accessor, validator, orchestration, mapper
use super::*;
use crate::diagnostic_producer::{
    TransactionAttempt, TransactionPhaseGuard, record_sqlite_failure, record_unacquired_release,
};
use crate::diagnostic_recorder::{
    DiagnosticPhase, OutcomeCertainty, SpanStart, SqliteDatabaseRole, SqliteEventIdentity,
    SqliteMeasurementGap, SqlitePathClass, SqlitePhaseEvidence, SqliteTransactionMode,
    SqliteTransactionPhase, process_recorder,
};
use crate::sqlite_observability::SqliteOperationObserver;
use oulipoly_agent_messenger::ReturnedArtifactRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const COMPLETED_TURN_RECOVERY_IDENTITY_LIMIT: usize = 100;

fn recovery_span(path: &Path, operation: &'static str, family: &'static str) -> SpanStart {
    SpanStart::new(operation, "state_sqlite").with_sqlite_identity(SqliteEventIdentity::new(
        SqliteDatabaseRole::State,
        if path == Path::new(":memory:") {
            SqlitePathClass::Memory
        } else {
            SqlitePathClass::ManagedFile
        },
        family,
    ))
}

fn observe_recovery_read<T>(
    span: impl FnOnce() -> SpanStart,
    read: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let observer = SqliteOperationObserver::process();
    let result = read();
    match &result {
        Ok(_) => {
            observer.record_success(
                span,
                DiagnosticPhase::Released,
                OutcomeCertainty::Terminal,
                |elapsed| {
                    SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::StatementExecution)
                        .with_execution(elapsed)
                        .with_gap(SqliteMeasurementGap::WriterAuthorityNotApplicable)
                        .with_gap(SqliteMeasurementGap::CommitNotApplicable)
                        .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                },
            );
        }
        Err(_) => {
            observer.record_failure_cause(
                span,
                "completed_turn_recovery_read_failed",
                OutcomeCertainty::StartedUnknown,
                |elapsed| {
                    SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::StatementExecution)
                        .with_execution(elapsed)
                        .with_gap(SqliteMeasurementGap::WriterAuthorityNotApplicable)
                        .with_gap(SqliteMeasurementGap::CommitNotApplicable)
                        .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                },
            );
        }
    }
    result
}
const COMPLETED_TURN_RECOVERY_ROW_SQL: &str =
    "SELECT c.invocation_uuid,c.settlement_id,c.owner_json,c.effects_json,
            c.context_json,c.content_sha256,c.committed_at IS NOT NULL,
            c.tails_json,i.provider_name,i.provider_session_id,i.session_id
     FROM completed_turns c INDEXED BY completed_turns_recovery_pending
     JOIN invocations i ON i.id=c.invocation_id
     WHERE c.recovery_pending=1 AND c.invocation_id=?1";

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

/// A bounded raw-index page. The cursor is the last examined invocation row,
/// including an inconsistent finished tail skipped by the legacy projection.
/// Continue while `has_more` is true, even if `items` is empty. If an older
/// invocation acquires a new duty mid-scan, finish the pass and restart when
/// `restart_required` is true; new higher IDs do not starve the current pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletedTurnRecoveryPage {
    pub items: Vec<CompletedTurnRecoveryIdentity>,
    pub next_after: Option<i64>,
    pub has_more: bool,
    pub epoch: i64,
    pub restart_required: bool,
}

#[derive(Debug, Clone)]
struct CompletedTurnRecoveryDuty {
    invocation_uuid: String,
    settlement_id: String,
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
    fn completed_turn_recovery_duties_at(
        &self,
        invocation_id: i64,
    ) -> Result<Vec<CompletedTurnRecoveryDuty>, String> {
        let mut stmt = self
            .conn
            .prepare(COMPLETED_TURN_RECOVERY_ROW_SQL)
            .map_err(custody_error)?;
        let rows = stmt
            .query_map([invocation_id], |row| {
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
                    _provider_name,
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
                    let _provider_session = provider_session_id
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

    fn completed_turn_recovery_duty_at(
        &self,
        invocation_id: i64,
    ) -> Result<CompletedTurnRecoveryDuty, String> {
        self.completed_turn_recovery_duties_at(invocation_id)?
            .into_iter()
            .next()
            .ok_or_else(|| "completed_turn_pending_projection_inconsistent".into())
    }

    fn first_recovery_duty_id<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Option<i64>, String> {
        self.conn
            .query_row(sql, params, |row| row.get(0))
            .optional()
            .map_err(custody_error)
    }

    fn first_recovery_duty_for_target(
        &self,
        provider: &str,
        session: Option<&str>,
        chain: Option<&str>,
    ) -> Result<Option<CompletedTurnRecoveryDuty>, String> {
        let target_id = if let Some(session) = session {
            self.first_recovery_duty_id(
                "SELECT invocation_id FROM completed_turns INDEXED BY completed_turns_recovery_target
                 WHERE recovery_pending=1 AND recovery_provider_name=?1
                   AND recovery_provider_session=?2 LIMIT 1",
                params![provider, session],
            )?
        } else {
            self.first_recovery_duty_id(
                "SELECT invocation_id FROM completed_turns INDEXED BY completed_turns_recovery_target
                 WHERE recovery_pending=1 AND recovery_provider_name=?1 LIMIT 1",
                [provider],
            )?
        };
        let id = if target_id.is_some() || chain.is_none() {
            target_id
        } else {
            self.first_recovery_duty_id(
                "SELECT c.invocation_id FROM session_chain_segments s
                 CROSS JOIN completed_turns c INDEXED BY completed_turns_recovery_target
                 WHERE s.chain_id=?1 AND c.recovery_pending=1
                   AND c.recovery_provider_name=s.provider_name
                   AND c.recovery_provider_session=s.session_id LIMIT 1",
                [chain.unwrap()],
            )?
        };
        id.map(|id| self.completed_turn_recovery_duty_at(id))
            .transpose()
    }

    fn first_recovery_duty_for_session(
        &self,
        session: &str,
    ) -> Result<Option<CompletedTurnRecoveryDuty>, String> {
        let direct = self.first_recovery_duty_id(
            "SELECT invocation_id FROM completed_turns INDEXED BY completed_turns_recovery_session
             WHERE recovery_pending=1 AND recovery_provider_session=?1 LIMIT 1",
            [session],
        )?;
        let id = if direct.is_some() {
            direct
        } else {
            self.first_recovery_duty_id(
                "SELECT c.invocation_id FROM session_chain_segments target
                 CROSS JOIN session_chain_segments original
                 CROSS JOIN completed_turns c INDEXED BY completed_turns_recovery_target
                 WHERE target.session_id=?1 AND target.ended_at IS NULL
                   AND original.chain_id=target.chain_id AND c.recovery_pending=1
                   AND c.recovery_provider_name=original.provider_name
                   AND c.recovery_provider_session=original.session_id LIMIT 1",
                [session],
            )?
        };
        id.map(|id| self.completed_turn_recovery_duty_at(id))
            .transpose()
    }

    fn recovery_duty_for_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<CompletedTurnRecoveryDuty>, String> {
        self.first_recovery_duty_id(
            "SELECT invocation_id FROM completed_turns WHERE invocation_uuid=?1 AND recovery_pending=1",
            [uuid],
        )?
        .map(|id| self.completed_turn_recovery_duty_at(id))
        .transpose()
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
        let session = match scope {
            CompletedTurnMigrationScope::ExternalProviderWide => None,
            CompletedTurnMigrationScope::BuiltInExact => target_session,
        };
        Ok(self
            .first_recovery_duty_for_target(target_provider, session, Some(chain_id))?
            .map(|duty| vec![format!("{}[{}]", duty.invocation_uuid, duty.phase)])
            .unwrap_or_default())
    }

    fn refuse_completed_turn_migration_target_stage(
        &self,
        fence: &CompletedTurnMigrationFence,
        target_session: Option<&str>,
        scope: CompletedTurnMigrationScope,
        stage: CompletedTurnMigrationStage,
    ) -> Result<(), String> {
        observe_recovery_read(
            || {
                let mut start = recovery_span(
                    self.path(),
                    "completed_turn_migration_target_recheck",
                    "completed_turn.recovery.target_recheck",
                )
                .with_hashed_correlation("chain_id", &fence.chain_id)
                .with_hashed_correlation("provider_name", &fence.target_provider)
                .with_identifier("migration_stage", format!("{stage:?}"))
                .with_identifier("migration_scope", format!("{scope:?}"));
                if let Some(session) = target_session {
                    start = start.with_hashed_correlation("session_id", session);
                }
                start
            },
            || {
                self.refuse_completed_turn_migration_target_stage_unobserved(
                    fence,
                    target_session,
                    scope,
                    stage,
                )
            },
        )
    }

    fn refuse_completed_turn_migration_target_stage_unobserved(
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
        let created_at = Self::current_rfc3339_timestamp();
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
        tx.execute(
            "INSERT INTO completed_turns (
            invocation_id,invocation_uuid,settlement_id,owner_json,effects_json,
            context_json,content_sha256,created_at,updated_at,retention_status
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8,'pending')",
            params![
                row,
                invocation.invocation_uuid,
                id,
                owner,
                effects_json,
                context_json,
                digest,
                created_at
            ],
        )
        .map_err(custody_error)?;
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

    pub fn completed_turns_for_session(&self, session: &str) -> Result<Vec<String>, String> {
        let mut ids = Vec::new();
        for sql in [
            "SELECT invocation_id FROM completed_turns INDEXED BY completed_turns_recovery_session
             WHERE recovery_pending=1 AND recovery_provider_session=?1",
            "SELECT c.invocation_id FROM session_chain_segments target
             CROSS JOIN session_chain_segments original
             CROSS JOIN completed_turns c INDEXED BY completed_turns_recovery_target
             WHERE target.session_id=?1 AND target.ended_at IS NULL
               AND original.chain_id=target.chain_id AND c.recovery_pending=1
               AND c.recovery_provider_name=original.provider_name
               AND c.recovery_provider_session=original.session_id",
        ] {
            let mut stmt = self.conn.prepare(sql).map_err(custody_error)?;
            ids.extend(
                stmt.query_map([session], |row| row.get::<_, i64>(0))
                    .map_err(custody_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(custody_error)?,
            );
        }
        ids.sort_unstable();
        ids.dedup();
        let mut pending = Vec::with_capacity(ids.len());
        for id in ids {
            // Preserve the legacy list's permissive handling of a stale
            // recovery projection. Refusal paths treat it as an error.
            if let Some(duty) = self
                .completed_turn_recovery_duties_at(id)?
                .into_iter()
                .next()
            {
                pending.push(format!("{}[{}]", duty.invocation_uuid, duty.phase));
            }
        }
        Ok(pending)
    }

    /// A resolver's chain choice is narrower than a native session string.
    /// Verify the current provider/chain/session tuple in State before using it
    /// to scope custody; a caller-supplied UUID alone is never authority.
    pub fn refuse_completed_turn_resolved_resume(
        &self,
        resolved: &ResolvedResume,
    ) -> Result<(), String> {
        observe_recovery_read(
            || {
                recovery_span(
                    self.path(),
                    "completed_turn_resolved_resume_lookup",
                    "completed_turn.recovery.resolved_lookup",
                )
                .with_hashed_correlation("chain_id", &resolved.chain_id)
                .with_hashed_correlation("session_id", &resolved.active_session_id)
                .with_hashed_correlation("provider_name", &resolved.active_provider)
            },
            || self.refuse_completed_turn_resolved_resume_unobserved(resolved),
        )
    }

    fn refuse_completed_turn_resolved_resume_unobserved(
        &self,
        resolved: &ResolvedResume,
    ) -> Result<(), String> {
        self.validate_resolved_resume_identity(resolved)?;
        let duty = self.first_recovery_duty_for_target(
            &resolved.active_provider,
            Some(&resolved.active_session_id),
            Some(&resolved.chain_id),
        )?;
        Self::refuse_pending_completed_turns(
            duty.into_iter()
                .map(|d| format!("{}[{}]", d.invocation_uuid, d.phase))
                .collect(),
        )
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
        observe_recovery_read(
            || {
                let mut start = recovery_span(
                    self.path(),
                    "completed_turn_migration_target_lookup",
                    "completed_turn.recovery.target_lookup",
                )
                .with_hashed_correlation("chain_id", &resolved.chain_id)
                .with_hashed_correlation("provider_name", target_provider);
                if let Some(session) = target_session {
                    start = start.with_hashed_correlation("session_id", session);
                }
                start
            },
            || {
                self.refuse_completed_turn_migration_target_unobserved(
                    resolved,
                    target_provider,
                    target_session,
                )
            },
        )
    }

    fn refuse_completed_turn_migration_target_unobserved(
        &self,
        resolved: &ResolvedResume,
        target_provider: &str,
        target_session: Option<&str>,
    ) -> Result<(), String> {
        self.validate_resolved_resume_identity(resolved)?;
        let duty = self.first_recovery_duty_for_target(
            target_provider,
            target_session,
            Some(&resolved.chain_id),
        )?;
        if let Some(duty) = duty {
            Err(format!(
                "completed_turn_target_pending: {}[{}]; target={}/{}; no transcript publication, segment rotation, external materialization, or provider execution",
                duty.invocation_uuid,
                duty.phase,
                target_provider,
                target_session.unwrap_or("<provider-wide-before-execution>")
            ))
        } else {
            Ok(())
        }
    }

    pub fn refuse_completed_turn_resume(&self, session: &str) -> Result<(), String> {
        observe_recovery_read(
            || {
                recovery_span(
                    self.path(),
                    "completed_turn_resume_lookup",
                    "completed_turn.recovery.session_lookup",
                )
                .with_hashed_correlation("session_id", session)
            },
            || self.refuse_completed_turn_resume_unobserved(session),
        )
    }

    fn refuse_completed_turn_resume_unobserved(&self, session: &str) -> Result<(), String> {
        let duty = self.first_recovery_duty_for_session(session)?;
        Self::refuse_pending_completed_turns(
            duty.into_iter()
                .map(|d| format!("{}[{}]", d.invocation_uuid, d.phase))
                .collect(),
        )
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
        let start = recovery_span(
            self.path(),
            "completed_turn_manual_resume",
            "completed_turn.manual_resume.state",
        )
        .with_sqlite_identity(
            SqliteEventIdentity::new(
                SqliteDatabaseRole::State,
                if self.path() == Path::new(":memory:") {
                    SqlitePathClass::Memory
                } else {
                    SqlitePathClass::ManagedFile
                },
                "completed_turn.manual_resume.state",
            )
            .with_transaction_mode(SqliteTransactionMode::Immediate),
        )
        .with_hashed_correlation("session_id", session);
        recorder.with_deferred_requested_span(start, |state_span| {
            let attempt = TransactionAttempt::start();
            let reservation = match sqlite::Transaction::new_unchecked(
                &self.conn,
                sqlite::TransactionBehavior::Immediate,
            ) {
                Ok(tx) => tx,
                Err(error) => {
                    record_sqlite_failure(state_span, &error, attempt);
                    record_unacquired_release(state_span);
                    return Err(custody_error(error));
                }
            };
            let mut state_phases = TransactionPhaseGuard::acquired(state_span, attempt);
            #[cfg(test)]
            tests::WITH_MANUAL_RESERVATION.with_borrow_mut(|hook| {
                if let Some(hook) = hook.take() {
                    hook();
                }
            });
            let mut crossed_to_sidecar = false;
            let result = (|| {
                // This writer reservation is the manual State authority boundary.
                // Refusal seeks the target projection; unrelated pending duties never
                // lengthen the State -> sidecar crossing.
                let lookup_started = std::time::Instant::now();
                let pending_result = (|| {
                    if let Some(resolved) = resolved {
                        self.validate_resolved_resume_identity(resolved)?;
                        self.first_recovery_duty_for_target(
                            &resolved.active_provider,
                            Some(&resolved.active_session_id),
                            Some(&resolved.chain_id),
                        )
                    } else {
                        // Legacy callers have no authenticated chain distinction.
                        self.first_recovery_duty_for_session(session)
                    }
                })();
                let lookup_elapsed = lookup_started.elapsed();
                let lookup_evidence =
                    SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::StatementExecution)
                        .with_execution(lookup_elapsed)
                        .with_total_elapsed(lookup_elapsed)
                        .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                        .with_gap(SqliteMeasurementGap::CommitNotApplicable);
                let lookup_observation = if pending_result.is_ok() {
                    crate::diagnostic_recorder::PhaseObservation::terminal()
                        .with_sqlite_evidence(lookup_evidence)
                } else {
                    crate::diagnostic_recorder::PhaseObservation::started_unknown()
                        .with_cause("manual_resume_target_lookup_failed")
                        .with_sqlite_evidence(lookup_evidence)
                };
                let _ = state_span.record_deferred_completed_child(
                    recovery_span(
                        self.path(),
                        "completed_turn_manual_target_lookup",
                        "completed_turn.recovery.manual_target_lookup",
                    )
                    .with_hashed_correlation("session_id", session),
                    lookup_elapsed,
                    if pending_result.is_ok() {
                        DiagnosticPhase::Released
                    } else {
                        DiagnosticPhase::Failed
                    },
                    lookup_observation,
                );
                let pending = pending_result?;
                Self::refuse_pending_completed_turns(
                    pending
                        .into_iter()
                        .map(|duty| format!("{}[{}]", duty.invocation_uuid, duty.phase))
                        .collect(),
                )?;
                let state_path = self
                    .completion_authority_state_path()
                    .ok_or("completed_turn_state_identity_unavailable")?;
                let path = crate::mailbox::MailboxDb::path_for_state_db(state_path);
                crossed_to_sidecar = true;
                let sidecar_start =
                    SpanStart::new("completed_turn_manual_resume_sidecar", "pid_mailbox_sqlite")
                        .with_sqlite_identity(SqliteEventIdentity::new(
                            SqliteDatabaseRole::PidMailbox,
                            SqlitePathClass::ManagedFile,
                            "completed_turn.manual_resume.sidecar_crossing",
                        ))
                        .with_diagnostic_id(state_span.diagnostic_id().clone())
                        .with_parent_span_id(state_span.span_id().clone())
                        .with_hashed_correlation("session_id", session);
                state_span.with_deferred_requested_span(sidecar_start, |sidecar_span| {
                    let crossing_started = std::time::Instant::now();
                    let result = (|| {
                        let authority =
                            crate::mailbox::MailboxAuthorityFence::try_acquire(&path)
                                .map_err(|e| format!("manual_resume_authority_unavailable: {e}"))?;
                        let mut sidecar =
            crate::mailbox::MailboxDb::open_existing_for_completion_authority_deferred(
                &authority, &recorder,
            )?;
                        // The sidecar has at most one claim for this session. Read its exact
                        // identity, seek that completed turn by UUID, and require the sidecar
                        // writer to recheck the identity before any release. A changed claim
                        // is a retry, never an inferred absence of pending recovery.
                        let expected_claim = sidecar.manual_wake_claim_identity(session)?;
                        #[cfg(test)]
                        tests::AFTER_MANUAL_CLAIM_PEEK.with_borrow_mut(|hook| {
                            if let Some(hook) = hook.take() {
                                hook();
                            }
                        });
                        let exact_duty = expected_claim
                            .as_ref()
                            .and_then(|claim| claim.wake_invocation_uuid.as_deref())
                            .map(|uuid| {
                                let lookup_started = std::time::Instant::now();
                                let duty = self.recovery_duty_for_uuid(uuid);
                                let elapsed = lookup_started.elapsed();
                                let evidence = SqlitePhaseEvidence::for_phase(
                                    SqliteTransactionPhase::StatementExecution,
                                )
                                .with_execution(elapsed)
                                .with_total_elapsed(elapsed)
                                .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed)
                                .with_gap(SqliteMeasurementGap::CommitNotApplicable);
                                let observation = if duty.is_ok() {
                                    crate::diagnostic_recorder::PhaseObservation::terminal()
                                        .with_sqlite_evidence(evidence)
                                } else {
                                    crate::diagnostic_recorder::PhaseObservation::started_unknown()
                                        .with_cause("manual_resume_exact_claim_recheck_failed")
                                        .with_sqlite_evidence(evidence)
                                };
                                let _ = state_span.record_deferred_completed_child(
                                    recovery_span(
                                        self.path(),
                                        "completed_turn_manual_exact_claim_recheck",
                                        "completed_turn.recovery.exact_claim_recheck",
                                    )
                                    .with_hashed_correlation("session_id", session)
                                    .with_hashed_correlation("invocation_uuid", uuid),
                                    elapsed,
                                    if duty.is_ok() {
                                        DiagnosticPhase::Released
                                    } else {
                                        DiagnosticPhase::Failed
                                    },
                                    observation,
                                );
                                duty
                            })
                            .transpose()?
                            .flatten();
                        let retained_claim = exact_duty.and_then(|duty| {
                            let claim = expected_claim.as_ref()?;
                            (duty.original_wake_claim.as_deref()
                                == Some(claim.claim_token.as_str()))
                            .then(|| crate::mailbox::RetainedWakeClaim {
                                invocation_uuid: duty.invocation_uuid,
                                settlement_id: duty.settlement_id,
                                claim_token: claim.claim_token.clone(),
                                phase: duty.phase,
                            })
                        });
                        sidecar.coordinate_manual_resume_without_wait(
                            session,
                            &retained_claim.into_iter().collect::<Vec<_>>(),
                            &expected_claim,
                            sidecar_span,
                        )
                    })();
                    let evidence = SqlitePhaseEvidence::for_phase(SqliteTransactionPhase::Released)
                        .with_total_elapsed(crossing_started.elapsed())
                        .with_gap(SqliteMeasurementGap::WriterWaitAndExecutionNotSeparable)
                        .with_gap(SqliteMeasurementGap::ExecutionNotExposedByApi)
                        .with_gap(SqliteMeasurementGap::CommitNotApplicable)
                        .with_gap(SqliteMeasurementGap::RowsExaminedNotExposed);
                    let observation = match &result {
                        Ok(_) => crate::diagnostic_recorder::PhaseObservation::terminal()
                            .with_sqlite_evidence(evidence),
                        Err(_) => crate::diagnostic_recorder::PhaseObservation::started_unknown()
                            .with_cause("manual_resume_sidecar_crossing_failed")
                            .with_sqlite_evidence(evidence),
                    };
                    let _ = sidecar_span.record(
                        if result.is_ok() {
                            DiagnosticPhase::Released
                        } else {
                            DiagnosticPhase::Failed
                        },
                        observation,
                    );
                    result
                })
            })();
            if result.is_err() && !crossed_to_sidecar {
                state_phases.failed("manual_resume_state_refused");
            }
            drop(reservation);
            state_phases.release_after_rollback();
            result
        })
    }

    /// Compatibility read for small queues. Refuse to return a truncated list;
    /// callers handling more than one page must use the explicit cursor API.
    pub fn completed_turn_identities(&self) -> Result<Vec<CompletedTurnRecoveryIdentity>, String> {
        let page = self.completed_turn_identity_page(None, None)?;
        if page.has_more {
            return Err(
                "completed_turn_recovery_page_required: pending work exceeds one page".into(),
            );
        }
        Ok(page.items)
    }

    /// Read at most 100 pending index entries under a read snapshot. A later
    /// page uses the first page's epoch. Finish the current pass on an epoch
    /// change, then restart from the beginning to include an older admission.
    pub fn completed_turn_identity_page(
        &self,
        after: Option<i64>,
        expected_epoch: Option<i64>,
    ) -> Result<CompletedTurnRecoveryPage, String> {
        observe_recovery_read(
            || {
                recovery_span(
                    self.path(),
                    "completed_turn_recovery_page",
                    "completed_turn.recovery.page",
                )
                .with_identifier("cursor_after_id", after.unwrap_or(0).to_string())
                .with_identifier("expected_epoch", expected_epoch.unwrap_or(0).to_string())
            },
            || self.completed_turn_identity_page_unobserved(after, expected_epoch),
        )
    }

    fn completed_turn_identity_page_unobserved(
        &self,
        after: Option<i64>,
        expected_epoch: Option<i64>,
    ) -> Result<CompletedTurnRecoveryPage, String> {
        let after = after.unwrap_or(0);
        if after < 0
            || after > 0 && expected_epoch.is_none()
            || after == 0 && expected_epoch.is_some()
            || expected_epoch.is_some_and(|epoch| epoch < 0)
        {
            return Err("completed_turn_recovery_cursor_invalid".into());
        }
        let _snapshot =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Deferred)
                .map_err(custody_error)?;
        let current_epoch: i64 = self
            .conn
            .query_row(
                "SELECT epoch FROM completed_turn_recovery_epoch WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(custody_error)?;
        let epoch = expected_epoch.unwrap_or(current_epoch);
        let restart_required = epoch != current_epoch;
        let mut stmt = self.conn.prepare(
            "SELECT invocation_id FROM completed_turns INDEXED BY completed_turns_recovery_pending
             WHERE recovery_pending=1 AND invocation_id>?1
             ORDER BY invocation_id LIMIT ?2",
        ).map_err(custody_error)?;
        let ids = stmt
            .query_map(
                params![after, (COMPLETED_TURN_RECOVERY_IDENTITY_LIMIT + 1) as i64],
                |row| row.get::<_, i64>(0),
            )
            .map_err(custody_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(custody_error)?;
        let has_more = ids.len() > COMPLETED_TURN_RECOVERY_IDENTITY_LIMIT;
        let examined = &ids[..ids.len().min(COMPLETED_TURN_RECOVERY_IDENTITY_LIMIT)];
        let mut items = Vec::with_capacity(examined.len());
        for id in examined {
            if let Some(duty) = self
                .completed_turn_recovery_duties_at(*id)?
                .into_iter()
                .next()
            {
                items.push(CompletedTurnRecoveryIdentity {
                    invocation_uuid: duty.invocation_uuid,
                    phase: duty.phase,
                });
            }
        }
        Ok(CompletedTurnRecoveryPage {
            items,
            next_after: has_more.then(|| *examined.last().expect("full page has a last row")),
            has_more,
            epoch,
            restart_required,
        })
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

    /// Returns false when this exact committed turn is already terminal. A
    /// replay must not replace its final tails with a new pending marker.
    pub fn record_completed_turn_tails(
        &self,
        uuid: &str,
        settlement: &str,
        tails: &serde_json::Value,
    ) -> Result<bool, String> {
        let recovery_pending = i64::from(!completed_turn_tails_finished(tails));
        let transition_at = Self::current_rfc3339_timestamp();
        let changed = self
            .conn
            .execute(
                "UPDATE completed_turns
             SET tails_json=?3,
                 recovery_pending=?4,
                 updated_at=?5,
                 closed_at=CASE WHEN ?4=0 THEN COALESCE(closed_at,?5) ELSE closed_at END,
                 retention_eligible_at=CASE
                   WHEN ?4=0 AND created_at IS NOT NULL
                    AND julianday(COALESCE(closed_at,?5))>=julianday(created_at)
                   THEN COALESCE(closed_at,?5) ELSE NULL END,
                 retention_status=CASE
                   WHEN ?4=1 THEN 'pending'
                   WHEN created_at IS NULL THEN 'legacy_unknown'
                   WHEN julianday(COALESCE(closed_at,?5))<julianday(created_at)
                   THEN 'clock_anomaly' ELSE 'eligible' END
             WHERE invocation_uuid=?1 AND settlement_id=?2
               AND committed_at IS NOT NULL AND recovery_pending=1",
                params![
                    uuid,
                    settlement,
                    encode(tails)?,
                    recovery_pending,
                    transition_at
                ],
            )
            .map_err(custody_error)?;
        if changed == 1 {
            return Ok(true);
        }
        let current: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT recovery_pending,tails_json FROM completed_turns
                 WHERE invocation_uuid=?1 AND settlement_id=?2 AND committed_at IS NOT NULL",
                params![uuid, settlement],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(custody_error)?;
        match current {
            Some((0, stored))
                if completed_turn_tails_finished(
                    &serde_json::from_str::<serde_json::Value>(&stored)
                        .map_err(|error| error.to_string())?,
                ) =>
            {
                Ok(false)
            }
            Some((0, _)) => Err("completed_turn_terminal_tails_inconsistent".into()),
            _ => Err("completed_turn_not_committed".into()),
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
    use std::collections::HashSet;
    use std::sync::mpsc;
    use std::time::Duration;

    thread_local! {
        pub(super) static BEFORE_MANUAL_RESERVATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
        pub(super) static WITH_MANUAL_RESERVATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
        pub(super) static AFTER_MANUAL_CLAIM_PEEK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
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
                deferred_queue_capacity: 64,
                ..RecorderConfig::default()
            },
        )
        .unwrap();
        let release_writer = recorder.block_writer_for_test().unwrap();
        let worker_recorder = recorder.clone();
        let (completed, completion) = mpsc::channel();
        let (entered, observation_entered) = mpsc::channel();
        let (left, observation_left) = mpsc::channel();

        let worker = std::thread::spawn(move || {
            let result = crate::mailbox::with_test_completion_open_observation(
                move |after_record| {
                    let _ = if after_record {
                        left.send(())
                    } else {
                        entered.send(())
                    };
                },
                || {
                    with_test_process_recorder(worker_recorder, || {
                        with_test_process_policy(SqliteObservationPolicy::all(), || {
                            state.coordinate_manual_resume("session")
                        })
                    })
                },
            );
            completed.send(result).unwrap();
        });

        observation_entered
            .recv_timeout(Duration::from_secs(5))
            .expect("manual-resume sidecar open reached its recorder handoff");
        let observation_finished = observation_left.recv_timeout(Duration::from_millis(250));
        release_writer.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            observation_finished.is_ok(),
            "manual-resume sidecar open waited for recorder progress while holding State"
        );
        assert_eq!(
            completion.recv().unwrap().unwrap(),
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
    fn manual_resume_reports_state_and_sidecar_phases_after_exact_release() {
        let (directory, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        mailbox
            .connection()
            .execute(
                "UPDATE session_wake_claim SET claim_token=?1 WHERE session_id='session'",
                ["claim-secret-sentinel"],
            )
            .unwrap();
        let recorder_root = directory.path().join("spans");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        with_test_process_recorder(recorder.clone(), || {
            assert_eq!(
                state.coordinate_manual_resume("session").unwrap(),
                crate::mailbox::ManualWakeCoordination::Released
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let events: Vec<_> = report.events.iter().map(|record| &record.event).collect();
        let state_release = events
            .iter()
            .find(|event| {
                event.operation == "completed_turn_manual_resume"
                    && event.phase == DiagnosticPhase::Released
            })
            .unwrap();
        let state_evidence = state_release.observation.sqlite.as_ref().unwrap();
        assert!(state_evidence.writer_authority_acquisition_micros.is_some());
        assert!(state_evidence.execution_micros.is_some());
        assert!(
            state_evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::CommitNotApplicable)
        );
        let sidecar_commit = events
            .iter()
            .find(|event| {
                event.operation == "completed_turn_manual_resume_sidecar_write"
                    && event.phase == DiagnosticPhase::Committed
            })
            .unwrap();
        assert_eq!(
            sidecar_commit.sqlite.as_ref().unwrap().database_role,
            SqliteDatabaseRole::PidMailbox
        );
        assert!(
            sidecar_commit
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .commit_micros
                .is_some()
        );
        assert!(events.iter().any(|event| {
            event.operation == "completed_turn_manual_resume_sidecar"
                && event.phase == DiagnosticPhase::Released
        }));
        let sidecar_crossing = events
            .iter()
            .find(|event| {
                event.operation == "completed_turn_manual_resume_sidecar"
                    && event.phase == DiagnosticPhase::Released
            })
            .unwrap();
        let sidecar_release = events
            .iter()
            .find(|event| {
                event.operation == "completed_turn_manual_resume_sidecar_write"
                    && event.phase == DiagnosticPhase::Released
            })
            .unwrap();
        println!(
            "manual-resume fixture state_begin_us={:?} state_body_us={:?} sidecar_crossing_us={:?} sidecar_begin_us={:?} sidecar_body_us={:?} sidecar_commit_us={:?}",
            state_evidence.writer_authority_acquisition_micros,
            state_evidence.execution_micros,
            sidecar_crossing
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .total_elapsed_micros,
            sidecar_release
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .writer_authority_acquisition_micros,
            sidecar_release
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .execution_micros,
            sidecar_commit
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .commit_micros,
        );
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
        let encoded = serde_json::to_string(&events).unwrap();
        assert!(!encoded.contains("claim-secret-sentinel"));
    }

    #[test]
    fn manual_resume_held_state_writer_reports_contention_and_preserves_claim() {
        let (directory, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        state.conn.busy_timeout(Duration::ZERO).unwrap();
        let holder = StateDb::open(state.path()).unwrap();
        holder.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let recorder_root = directory.path().join("spans");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        with_test_process_recorder(recorder.clone(), || {
            assert!(state.coordinate_manual_resume("session").is_err());
        });
        holder.conn.execute_batch("ROLLBACK").unwrap();
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_some()
        );
        with_test_process_recorder(recorder.clone(), || {
            assert_eq!(
                state.coordinate_manual_resume("session").unwrap(),
                crate::mailbox::ManualWakeCoordination::Released
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(&recorder_root).inspect();
        assert!(report.events.iter().any(|record| {
            record.event.operation == "completed_turn_manual_resume"
                && record.event.phase == DiagnosticPhase::Contention
        }));
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn manual_resume_held_sidecar_writer_and_missing_sink_preserve_retry() {
        let (directory, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let path = crate::mailbox::MailboxDb::path_for_state_db(state.path());
        let holder = sqlite::Connection::open(&path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let recorder_root = directory.path().join("spans");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        with_test_process_recorder(recorder.clone(), || {
            assert!(state.coordinate_manual_resume("session").is_err());
        });
        with_test_process_recorder(FlightRecorder::disabled(), || {
            assert!(state.coordinate_manual_resume("session").is_err());
        });
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_some()
        );
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(&recorder_root).inspect();
        assert!(report.events.iter().any(|record| {
            record.event.operation == "completed_turn_manual_resume_sidecar_write"
                && record.event.phase == DiagnosticPhase::Contention
        }));
        assert!(report.events.iter().any(|record| {
            record.event.operation == "completed_turn_manual_resume"
                && record.event.phase == DiagnosticPhase::Released
        }));
        holder.execute_batch("ROLLBACK").unwrap();
        with_test_process_recorder(recorder.clone(), || {
            assert_eq!(
                state.coordinate_manual_resume("session").unwrap(),
                crate::mailbox::ManualWakeCoordination::Released
            );
            assert_eq!(
                state.coordinate_manual_resume("session").unwrap(),
                crate::mailbox::ManualWakeCoordination::Absent
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn recovery_page_records_early_failure_and_optional_fast_sample() {
        let (directory, state, _, _) = fixture();
        let recorder_root = directory.path().join("spans");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        with_test_process_recorder(recorder.clone(), || {
            with_test_process_policy(
                SqliteObservationPolicy {
                    slow_threshold: Duration::from_secs(60),
                    ..SqliteObservationPolicy::default()
                },
                || {
                    assert!(state.completed_turn_identity_page(Some(1), None).is_err());
                    assert!(state.completed_turn_identity_page(None, None).is_ok());
                },
            );
            with_test_process_policy(SqliteObservationPolicy::all(), || {
                assert!(state.completed_turn_identity_page(None, None).is_ok());
            });
        });
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let page_events: Vec<_> = report
            .events
            .iter()
            .filter(|record| record.event.operation == "completed_turn_recovery_page")
            .collect();
        assert_eq!(page_events.len(), 2);
        assert!(
            page_events
                .iter()
                .any(|record| record.event.phase == DiagnosticPhase::Failed)
        );
        assert!(
            page_events
                .iter()
                .any(|record| record.event.phase == DiagnosticPhase::Released)
        );
        assert!(page_events.iter().all(|record| {
            record
                .event
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .rows_examined
                .is_none()
        }));
    }

    #[test]
    fn exact_migration_recheck_retains_stage_and_target_provenance() {
        let (directory, state, _, effects) = fixture();
        let resolved = bind_for_migration(&state, &effects);
        let recorder_root = directory.path().join("spans");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        with_test_process_recorder(recorder.clone(), || {
            with_test_process_policy(SqliteObservationPolicy::all(), || {
                let fence = state
                    .begin_completed_turn_migration(
                        &resolved,
                        "external-target",
                        None,
                        CompletedTurnMigrationScope::ExternalProviderWide,
                        CompletedTurnMigrationStage::ExternalBeforeProvider,
                    )
                    .unwrap();
                state
                    .recheck_completed_turn_migration_exact_target(
                        &fence,
                        "external-session",
                        CompletedTurnMigrationStage::ExternalAfterProviderBeforeHostApply,
                    )
                    .unwrap();
            });
        });
        recorder.drain_deferred_for_test().unwrap();
        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let exact = report
            .events
            .iter()
            .find(|record| {
                record.event.operation == "completed_turn_migration_target_recheck"
                    && record
                        .event
                        .correlations
                        .get("migration_scope")
                        .map(String::as_str)
                        == Some("BuiltInExact")
            })
            .unwrap();
        assert_eq!(exact.event.phase, DiagnosticPhase::Released);
        assert_eq!(
            exact
                .event
                .correlations
                .get("migration_stage")
                .map(String::as_str),
            Some("ExternalAfterProviderBeforeHostApply")
        );
        assert!(
            exact
                .event
                .correlations
                .get("provider_name")
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            exact
                .event
                .correlations
                .get("session_id")
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            exact
                .event
                .observation
                .sqlite
                .as_ref()
                .unwrap()
                .rows_examined
                .is_none()
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
        let query = format!("EXPLAIN QUERY PLAN {COMPLETED_TURN_RECOVERY_ROW_SQL}");
        let details = state
            .conn
            .prepare(&query)
            .unwrap()
            .query_map([0], |row| row.get::<_, String>(3))
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

    fn seed_pending_recovery(
        state: &StateDb,
        original: &CompletedTurnEffects,
        provider: &str,
        session: &str,
    ) -> (String, i64) {
        let uuid = Uuid::new_v4().to_string();
        let id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.clone(),
                model_name: "model".into(),
                provider_name: provider.into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        insert_pending_recovery(state, original, &uuid, id, session);
        (uuid, id)
    }

    fn insert_pending_recovery(
        state: &StateDb,
        original: &CompletedTurnEffects,
        uuid: &str,
        id: i64,
        session: &str,
    ) {
        let mut effects = original.clone();
        effects.invocation_row_id = id;
        effects.session_id = session.into();
        effects.turn_generation_id = uuid.to_owned();
        effects.returned_artifacts.clear();
        let context = serde_json::json!({"provider_session":session});
        let effects_json = encode(&effects).unwrap();
        let context_json = encode(&context).unwrap();
        let owner: Option<String> = None;
        let digest = fingerprint(&owner, &effects_json, &context_json).unwrap();
        state
            .conn
            .execute(
                "INSERT INTO completed_turns
             (invocation_id,invocation_uuid,settlement_id,owner_json,effects_json,
              context_json,content_sha256,created_at,updated_at,retention_status)
             VALUES (?1,?2,?3,NULL,?4,?5,?6,?7,?7,'pending')",
                params![
                    id,
                    uuid,
                    Uuid::new_v4().to_string(),
                    effects_json,
                    context_json,
                    digest,
                    chrono::Utc::now().to_rfc3339()
                ],
            )
            .unwrap();
    }

    #[test]
    fn recovery_pages_progress_with_new_arrivals_and_restart_for_older_duty() {
        let (_dir, state, _uuid, effects) = fixture();
        let delayed = Uuid::new_v4().to_string();
        let delayed_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: delayed.clone(),
                model_name: "model".into(),
                provider_name: "unrelated".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let expected: Vec<_> = (0..205)
            .map(|_| seed_pending_recovery(&state, &effects, "unrelated", "other").0)
            .collect();
        assert!(
            state
                .completed_turn_identities()
                .unwrap_err()
                .contains("page_required")
        );
        let page_plan = state
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT invocation_id FROM completed_turns
             INDEXED BY completed_turns_recovery_pending
             WHERE recovery_pending=1 AND invocation_id>?1
             ORDER BY invocation_id LIMIT ?2",
            )
            .unwrap()
            .query_map(params![0, 101], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            page_plan.iter().any(|part| {
                part.contains("SEARCH completed_turns USING")
                    && part.contains("completed_turns_recovery_pending")
            }),
            "{page_plan:?}"
        );
        let first = state.completed_turn_identity_page(None, None).unwrap();
        assert_eq!(first.items.len(), 100);
        assert!(first.has_more);
        assert!(first.next_after.is_some());
        assert!(!first.restart_required);
        // Higher IDs and a target-only mutation cannot starve this pass.
        let arrival = seed_pending_recovery(&state, &effects, "unrelated", "other").0;
        state
            .conn
            .execute(
                "UPDATE invocations SET provider_session_id='changed' WHERE invocation_uuid=?1",
                [&expected[0]],
            )
            .unwrap();
        let second = state
            .completed_turn_identity_page(first.next_after, Some(first.epoch))
            .unwrap();
        assert_eq!(second.items.len(), 100);
        assert!(!second.restart_required);
        state
            .conn
            .execute(
                "UPDATE completed_turns SET committed_at='2026-09-24T00:00:00Z',
             closed_at='2026-09-24T00:00:00Z', recovery_pending=0,
             tails_json=?1 WHERE invocation_uuid=?2",
                params![
                    serde_json::json!({
                        "native":"complete_or_standalone", "delivery":"complete",
                        "idle":"no_runtime", "wake":"no_mailbox"
                    })
                    .to_string(),
                    expected[0]
                ],
            )
            .unwrap();
        // This completion uses an invocation ID older than the cursor. The
        // current pass continues fairly, then explicitly requires a restart.
        insert_pending_recovery(&state, &effects, &delayed, delayed_id, "other");
        let third = state
            .completed_turn_identity_page(second.next_after, Some(first.epoch))
            .unwrap();
        assert!(!third.has_more);
        assert!(third.restart_required);
        let mut found = HashSet::new();
        for page in [&first, &second, &third] {
            for identity in &page.items {
                assert!(
                    found.insert(identity.invocation_uuid.clone()),
                    "duplicate recovery identity"
                );
                assert_eq!(identity.phase, "settlement_pending");
            }
        }
        assert_eq!(found.len(), 206);
        assert!(expected.iter().all(|uuid| found.contains(uuid)));
        assert!(found.contains(&arrival));
        assert!(!found.contains(&delayed));

        let mut restart_found = HashSet::new();
        let mut after = None;
        let mut epoch = None;
        let mut pages = 0;
        loop {
            let page = state.completed_turn_identity_page(after, epoch).unwrap();
            pages += 1;
            assert!(!page.restart_required);
            for identity in page.items {
                assert!(restart_found.insert(identity.invocation_uuid));
            }
            if !page.has_more {
                break;
            }
            after = page.next_after;
            epoch = Some(page.epoch);
        }
        assert_eq!(pages, 3);
        assert_eq!(restart_found.len(), 206);
        assert!(restart_found.contains(&delayed));
        assert!(!restart_found.contains(&expected[0]));
    }

    #[test]
    fn target_lookup_seeks_pending_projection_after_many_unrelated_duties() {
        let (_dir, state, _uuid, effects) = fixture();
        for _ in 0..160 {
            seed_pending_recovery(&state, &effects, "unrelated", "other");
        }
        let (target, id) = seed_pending_recovery(&state, &effects, "fixture", "target");
        let query = "EXPLAIN QUERY PLAN SELECT invocation_id FROM completed_turns
            INDEXED BY completed_turns_recovery_target
            WHERE recovery_pending=1 AND recovery_provider_name=?1
              AND recovery_provider_session=?2 LIMIT 1";
        let plan = state
            .conn
            .prepare(query)
            .unwrap()
            .query_map(params!["fixture", "target"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|part| part.contains("completed_turns_recovery_target")),
            "{plan:?}"
        );
        let chain_plan = state
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT c.invocation_id FROM session_chain_segments s
             CROSS JOIN completed_turns c INDEXED BY completed_turns_recovery_target
             WHERE s.chain_id=?1 AND c.recovery_pending=1
               AND c.recovery_provider_name=s.provider_name
               AND c.recovery_provider_session=s.session_id LIMIT 1",
            )
            .unwrap()
            .query_map(["chain"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            chain_plan.iter().any(|part| {
                part.contains("SEARCH c USING") && part.contains("completed_turns_recovery_target")
            }),
            "{chain_plan:?}"
        );
        assert_eq!(
            state.completed_turns_for_session("target").unwrap(),
            vec![format!("{target}[settlement_pending]")]
        );
        assert!(
            state
                .refuse_completed_turn_resume("target")
                .unwrap_err()
                .contains(&target)
        );
        assert!(
            state
                .coordinate_manual_resume("target")
                .unwrap_err()
                .contains(&target)
        );
        state
            .conn
            .execute(
                "UPDATE invocations SET provider_session_id='moved' WHERE id=?1",
                [id],
            )
            .unwrap();
        assert!(state.refuse_completed_turn_resume("target").is_ok());
        assert!(
            state
                .refuse_completed_turn_resume("moved")
                .unwrap_err()
                .contains(&target)
        );
        let (tampered, _) = seed_pending_recovery(&state, &effects, "fixture", "tamper-old");
        state
            .conn
            .execute(
                "UPDATE completed_turns SET context_json=?1 WHERE invocation_uuid=?2",
                params![
                    serde_json::json!({"provider_session":"tamper-new"}).to_string(),
                    tampered
                ],
            )
            .unwrap();
        assert!(
            state
                .refuse_completed_turn_resume("tamper-new")
                .unwrap_err()
                .contains("completed_turn_corrupt_admission")
        );
    }

    #[test]
    fn many_pending_duties_do_not_extend_sidecar_wait_under_state() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        for _ in 0..160 {
            seed_pending_recovery(&state, &effects, "unrelated", "other");
        }
        let holder = sqlite::Connection::open(mailbox.path()).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        assert!(state.coordinate_manual_resume("session").is_err());
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "sidecar waited under State: {elapsed:?}"
        );
        let probe = sqlite::Connection::open(state.path()).unwrap();
        probe.busy_timeout(Duration::ZERO).unwrap();
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
            "bounded manual State-to-sidecar lock test elapsed_us={}",
            elapsed.as_micros()
        );
    }

    #[test]
    fn changed_sidecar_claim_after_exact_lookup_requires_retry() {
        let (_dir, state, uuid, effects) = fixture();
        let mailbox = manual_fixture(&state, &effects, &uuid);
        let path = mailbox.path().to_path_buf();
        AFTER_MANUAL_CLAIM_PEEK.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                let writer = sqlite::Connection::open(path).unwrap();
                writer
                    .execute(
                        "UPDATE session_wake_claim SET claimed_at='2001-01-01T00:00:00Z'
                     WHERE session_id='session'",
                        [],
                    )
                    .unwrap();
            }));
        });
        let error = state.coordinate_manual_resume("session").unwrap_err();
        assert!(
            error.contains("manual_resume_claim_changed_retry"),
            "{error}"
        );
        assert!(
            mailbox
                .wake_session_reader()
                .wake_claim("session")
                .unwrap()
                .is_some()
        );
        let probe = sqlite::Connection::open(state.path()).unwrap();
        probe.busy_timeout(Duration::ZERO).unwrap();
        probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
        assert_eq!(
            state.coordinate_manual_resume("session").unwrap(),
            crate::mailbox::ManualWakeCoordination::Released
        );
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
    fn completed_turn_tail_replay_preserves_terminal_state_after_restart() {
        let (dir, state, uuid, effects) = fixture();
        let settlement = admit(&state, &effects);
        let record = state.completed_turn(&uuid).unwrap().unwrap();
        state.settle_completed_turn(&record).unwrap();
        let pending = serde_json::json!({"native":"pending","delivery":"pending"});
        let finished = serde_json::json!({
            "native":"complete_or_standalone",
            "delivery":"complete",
            "idle":"complete",
            "wake":"no_pending_at_recheck"
        });
        assert!(
            state
                .record_completed_turn_tails(&uuid, &settlement, &pending)
                .unwrap()
        );
        drop(state);

        // A lost reply after the first tail write leaves a recoverable duty.
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        assert_eq!(state.completed_turn_identities().unwrap().len(), 1);
        assert!(
            state
                .record_completed_turn_tails(&uuid, &settlement, &pending)
                .unwrap()
        );
        assert!(
            state
                .record_completed_turn_tails(&uuid, &settlement, &finished)
                .unwrap()
        );
        let before: (String, String) = state
            .conn
            .query_row(
                "SELECT closed_at,updated_at FROM completed_turns WHERE invocation_uuid=?1",
                [&uuid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(
            !state
                .record_completed_turn_tails(&uuid, &settlement, &pending)
                .unwrap()
        );
        assert!(
            !state
                .record_completed_turn_tails(&uuid, &settlement, &finished)
                .unwrap()
        );
        assert_eq!(
            state.completed_turn(&uuid).unwrap().unwrap().tails,
            finished
        );
        assert!(state.completed_turn_identities().unwrap().is_empty());
        let after: (String, String, i64) = state
            .conn
            .query_row(
                "SELECT closed_at,updated_at,recovery_pending FROM completed_turns WHERE invocation_uuid=?1",
                [&uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(after, (before.0, before.1, 0));
        assert!(
            state
                .conn
                .execute(
                    "UPDATE completed_turns SET recovery_pending=1 WHERE invocation_uuid=?1",
                    [&uuid]
                )
                .unwrap_err()
                .to_string()
                .contains("terminal state cannot reopen")
        );
        assert!(
            state
                .record_completed_turn_tails(&uuid, "wrong-settlement", &pending)
                .unwrap_err()
                .contains("completed_turn_not_committed")
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
