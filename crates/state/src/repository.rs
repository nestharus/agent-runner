use super::db::{
    AccountRecord, BackfillReport, ChainPreview, CliProviderRecord, DbError, DiscoveredModel,
    InvocationRecord, InvocationStart, ModelParameter, ProviderRecord, QuotaRecord, QuotaWindow,
    QuotaWindowInput, ReadOnlyOpenError, ResumeError, StateDb,
};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionReason {
    Initial,
    Manual,
    QuotaThreshold,
    Exhausted,
    Imported,
}

impl TransitionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TransitionReason::Initial => "initial",
            TransitionReason::Manual => "manual",
            TransitionReason::QuotaThreshold => "quota_threshold",
            TransitionReason::Exhausted => "exhausted",
            TransitionReason::Imported => "imported",
        }
    }
}

pub trait InvocationRepository {
    fn start_invocation(&self, start: &InvocationStart) -> Result<i64, String>;
    fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        stderr_snippet: Option<&str>,
    ) -> Result<(), String>;
    fn update_session_capture(
        &self,
        id: i64,
        session_id: Option<&str>,
        method: &str,
    ) -> Result<(), String>;
    fn update_resume_acceptance(
        &self,
        id: i64,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String>;
    fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String>;
    fn list_invocation_children(&self, parent_id: i64) -> Result<Vec<InvocationRecord>, String>;
}

pub trait RoutingRepository {
    fn get_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<Option<ProviderRecord>, String>;
    fn recent_error_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<u64, String>;
    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String>;
    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String>;
    fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String>;
}

pub trait QuotaRepository {
    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String>;
    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String>;
    fn mark_exhausted(&self, provider_name: &str) -> Result<(), String>;
    fn upsert_quota_refresh(
        &self,
        provider_name: &str,
        windows: &[QuotaWindowInput],
    ) -> Result<(), String>;
    fn increment_calls_since_refresh(&self, provider_name: &str) -> Result<(), String>;
}

pub struct ResumeDbFacts {
    pub chain_id: String,
    pub inferred_model_name: Option<String>,
    pub active_provider: String,
    pub active_session_id: String,
}

pub trait SessionChainRepository {
    fn backfill_session_chains(&self) -> Result<BackfillReport, DbError>;
    fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError>;
    fn mint_imported_chain_if_absent(
        &self,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<(), DbError>;
    fn resolve_resume_facts(
        &self,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResumeDbFacts, ResumeError>;
    fn resume_previews(&self, input: &str) -> Result<Vec<ChainPreview>, DbError>;
    fn chain_id_for_segment(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError>;
    fn active_segment_id_for_chain_provider_session(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<i64>, DbError>;
    fn find_session_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Option<String>, String>;
}

pub struct SessionTurnReplacement {
    pub provider_name: String,
    pub session_id: String,
    pub chain_id: String,
    pub active_segment_id: i64,
    pub source_file: PathBuf,
    pub turns: Vec<SessionTurnReplacementTurn>,
}

pub struct SessionTurnReplacementTurn {
    pub turn_id: String,
    pub timestamp: String,
    pub role: String,
}

pub trait SessionTurnRepository {
    fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[super::db::SessionTurnIngest],
    ) -> Result<u64, String>;

    fn replace_session_turns(&self, replacement: &SessionTurnReplacement) -> Result<(), String>;
}

pub trait CliProviderRepository {
    fn upsert_cli_provider(&self, provider: &CliProviderRecord) -> Result<(), String>;
    fn list_cli_providers(&self) -> Result<Vec<CliProviderRecord>, String>;
    fn get_cli_provider(&self, cli_name: &str) -> Result<Option<CliProviderRecord>, String>;
    fn insert_account(&self, account: &AccountRecord) -> Result<(), String>;
    fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String>;
    fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String>;
}

pub trait DiscoveryRepository {
    fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String>;
    fn list_discovered_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, String>;
    fn delete_stale_models(&self, provider: &str, current_cli_version: &str)
    -> Result<u64, String>;
    fn upsert_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        param: &ModelParameter,
    ) -> Result<(), String>;
    fn list_model_parameters(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameter>, String>;
}

pub trait StateDbOpener {
    fn open(&self, path: &Path) -> Result<StateDb, String>;
    fn open_default(&self) -> Result<StateDb, String>;
    fn default_path(&self) -> Result<PathBuf, String>;
    fn open_read_only(&self, path: &Path) -> Result<StateDb, ReadOnlyOpenError>;
}

pub struct DefaultStateDbOpener;

impl StateDbOpener for DefaultStateDbOpener {
    fn open(&self, path: &Path) -> Result<StateDb, String> {
        StateDb::open(path)
    }

    fn open_default(&self) -> Result<StateDb, String> {
        StateDb::open_default()
    }

    fn default_path(&self) -> Result<PathBuf, String> {
        StateDb::default_path()
    }

    fn open_read_only(&self, path: &Path) -> Result<StateDb, ReadOnlyOpenError> {
        StateDb::open_read_only(path)
    }
}

impl InvocationRepository for StateDb {
    fn start_invocation(&self, start: &InvocationStart) -> Result<i64, String> {
        StateDb::start_invocation(self, start)
    }

    fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        stderr_snippet: Option<&str>,
    ) -> Result<(), String> {
        StateDb::finalize_invocation(self, id, success, exit_code, error_category, stderr_snippet)
    }

    fn update_session_capture(
        &self,
        id: i64,
        session_id: Option<&str>,
        method: &str,
    ) -> Result<(), String> {
        StateDb::update_session_capture(self, id, session_id, method)
    }

    fn update_resume_acceptance(
        &self,
        id: i64,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String> {
        StateDb::update_resume_acceptance(self, id, status, evidence)
    }

    fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String> {
        StateDb::get_invocation_by_uuid(self, uuid)
    }

    fn list_invocation_children(&self, parent_id: i64) -> Result<Vec<InvocationRecord>, String> {
        StateDb::list_invocation_children(self, parent_id)
    }
}

impl RoutingRepository for StateDb {
    fn get_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<Option<ProviderRecord>, String> {
        StateDb::get_provider(self, model_name, provider_name)
    }

    fn recent_error_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<u64, String> {
        StateDb::recent_error_count(self, model_name, provider_name, window_minutes)
            .map(|count| count.max(0) as u64)
    }

    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        StateDb::get_quota(self, provider_name)
    }

    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        StateDb::get_windows(self, provider_name)
    }

    fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        StateDb::count_assistant_turns_since(self, provider_name, since)
    }
}

impl QuotaRepository for StateDb {
    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        StateDb::get_quota(self, provider_name)
    }

    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        StateDb::get_windows(self, provider_name)
    }

    fn mark_exhausted(&self, provider_name: &str) -> Result<(), String> {
        StateDb::mark_exhausted(self, provider_name)
    }

    fn upsert_quota_refresh(
        &self,
        provider_name: &str,
        windows: &[QuotaWindowInput],
    ) -> Result<(), String> {
        StateDb::upsert_quota_refresh(self, provider_name, windows)
    }

    fn increment_calls_since_refresh(&self, provider_name: &str) -> Result<(), String> {
        StateDb::increment_calls_since_refresh(self, provider_name)
    }
}

impl SessionChainRepository for StateDb {
    fn backfill_session_chains(&self) -> Result<BackfillReport, DbError> {
        StateDb::backfill_session_chains(self)
    }

    fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError> {
        StateDb::open_chain_segment(
            self,
            chain_id,
            provider_name,
            session_id,
            started_at,
            reason,
        )
    }

    fn mint_imported_chain_if_absent(
        &self,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<(), DbError> {
        StateDb::mint_imported_chain_if_absent(
            self,
            provider_name,
            session_id,
            started_at,
            model_name,
        )
    }

    fn resolve_resume_facts(
        &self,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResumeDbFacts, ResumeError> {
        StateDb::resolve_resume_facts(self, input, model_override)
    }

    fn resume_previews(&self, input: &str) -> Result<Vec<ChainPreview>, DbError> {
        StateDb::resume_previews(self, input)
    }

    fn chain_id_for_segment(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError> {
        StateDb::chain_id_for_segment(self, provider_name, session_id)
    }

    fn active_segment_id_for_chain_provider_session(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<i64>, DbError> {
        StateDb::active_segment_id_for_chain_provider_session(
            self,
            chain_id,
            provider_name,
            session_id,
        )
    }

    fn find_session_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Option<String>, String> {
        StateDb::find_session_for_invocation_window(self, provider_name, started_at, finished_at)
    }
}

impl SessionTurnRepository for StateDb {
    fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[super::db::SessionTurnIngest],
    ) -> Result<u64, String> {
        StateDb::ingest_session_turns_batch(self, provider_name, turns)
    }

    fn replace_session_turns(&self, replacement: &SessionTurnReplacement) -> Result<(), String> {
        let tx = self
            .connection()
            .unchecked_transaction()
            .map_err(|e| format!("failed to begin db transaction: {e}"))?;
        tx.execute(
            "DELETE FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
            params![replacement.provider_name, replacement.session_id],
        )
        .map_err(|e| format!("failed to delete old turns: {e}"))?;
        let now = Utc::now().to_rfc3339();
        for turn in &replacement.turns {
            tx.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role,
                     parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 0, ?6, ?7)",
                params![
                    replacement.provider_name,
                    replacement.session_id,
                    turn.turn_id,
                    turn.timestamp,
                    turn.role,
                    replacement.source_file.to_string_lossy(),
                    now,
                ],
            )
            .map_err(|e| format!("failed to insert replacement turn: {e}"))?;
        }
        let last = replacement
            .turns
            .last()
            .ok_or_else(|| "cannot replace db with empty records".to_string())?;
        tx.execute(
            "UPDATE session_chain_segments
             SET last_turn_id = ?2
             WHERE id = ?1",
            params![replacement.active_segment_id, last.turn_id],
        )
        .map_err(|e| format!("failed to refresh active segment: {e}"))?;
        tx.execute(
            "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
            params![replacement.chain_id, last.timestamp],
        )
        .map_err(|e| format!("failed to refresh chain: {e}"))?;
        tx.commit()
            .map_err(|e| format!("failed to commit db replacement: {e}"))
    }
}

impl CliProviderRepository for StateDb {
    fn upsert_cli_provider(&self, provider: &CliProviderRecord) -> Result<(), String> {
        StateDb::upsert_cli_provider(self, provider)
    }

    fn list_cli_providers(&self) -> Result<Vec<CliProviderRecord>, String> {
        StateDb::list_cli_providers(self)
    }

    fn get_cli_provider(&self, cli_name: &str) -> Result<Option<CliProviderRecord>, String> {
        StateDb::get_cli_provider(self, cli_name)
    }

    fn insert_account(&self, account: &AccountRecord) -> Result<(), String> {
        StateDb::insert_account(self, account)
    }

    fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String> {
        StateDb::list_accounts(self, provider)
    }

    fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String> {
        StateDb::delete_account(self, id, provider)
    }
}

impl DiscoveryRepository for StateDb {
    fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String> {
        StateDb::upsert_discovered_model(self, model)
    }

    fn list_discovered_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, String> {
        StateDb::list_discovered_models(self, provider)
    }

    fn delete_stale_models(
        &self,
        provider: &str,
        current_cli_version: &str,
    ) -> Result<u64, String> {
        StateDb::delete_stale_models(self, provider, current_cli_version)
    }

    fn upsert_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        param: &ModelParameter,
    ) -> Result<(), String> {
        StateDb::upsert_model_parameter(self, model_name, provider, param)
    }

    fn list_model_parameters(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameter>, String> {
        StateDb::list_model_parameters(self, model_name, provider)
    }
}
