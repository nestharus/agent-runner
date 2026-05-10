use crate::deployment::DeploymentAwareOpener;
use crate::schema_probe::{self, SchemaProbeReport};
use crate::{
    AccountRecord, ChainPreview, CliProviderRecord, DbError, DiscoveredModel, InvocationRecord,
    InvocationStart, InvocationStatus, ModelParameter, ModelStore, ProviderRecord, QuotaRecord,
    QuotaWindow, QuotaWindowInput, ResolvedResume, ResumeError, SessionTurnCounts,
    SessionTurnIngest, StateDb,
};
use chrono::{DateTime, Utc};
use oulipoly_core::TransitionReason;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

static NEXT_DEPLOYMENT_AWARE_OPENER_TOKEN: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone)]
struct DeploymentAwareOpenerSlot {
    token: usize,
    opener: DeploymentAwareOpener,
}

#[derive(Debug)]
struct DeploymentAwareOpenerRegistration {
    token: usize,
}

thread_local! {
    static DEPLOYMENT_AWARE_OPENER: RefCell<Option<DeploymentAwareOpenerSlot>> = const { RefCell::new(None) };
}

/// Repository slice over invocation lifecycle rows.
///
/// Wraps existing `StateDb` methods; later AGE-8 WUs cut production callers
/// over to this trait without changing persistence behavior.
pub trait InvocationRepository {
    fn start_invocation(&self, start: InvocationStart) -> Result<InvocationRecord, String>;
    fn finalize_invocation(
        &self,
        invocation_uuid: &str,
        status: InvocationStatus,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String>;
    fn update_session_capture(
        &self,
        invocation_uuid: &str,
        session_id: Option<&str>,
        method: &str,
    ) -> Result<(), String>;
    fn update_resume_acceptance(
        &self,
        invocation_uuid: &str,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String>;
    fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String>;
    fn list_invocation_children(&self, parent_uuid: &str) -> Result<Vec<InvocationRecord>, String>;
}

impl InvocationRepository for StateDb {
    fn start_invocation(&self, start: InvocationStart) -> Result<InvocationRecord, String> {
        StateDb::start_invocation(self, &start)?;
        StateDb::get_invocation_by_uuid(self, &start.invocation_uuid)?.ok_or_else(|| {
            format!(
                "Invocation {} not found after insert",
                start.invocation_uuid
            )
        })
    }

    fn finalize_invocation(
        &self,
        invocation_uuid: &str,
        status: InvocationStatus,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String> {
        let record = StateDb::get_invocation_by_uuid(self, invocation_uuid)?
            .ok_or_else(|| format!("Invocation {invocation_uuid} not found"))?;
        let success = match status {
            InvocationStatus::Succeeded => true,
            InvocationStatus::Failed => false,
            InvocationStatus::Running | InvocationStatus::Legacy => {
                return Err(format!(
                    "Invocation {invocation_uuid} cannot be finalized with status {}",
                    status.as_str()
                ));
            }
        };
        StateDb::finalize_invocation(
            self,
            record.id,
            success,
            exit_code,
            error_category,
            terminal_reason,
        )
    }

    fn update_session_capture(
        &self,
        invocation_uuid: &str,
        session_id: Option<&str>,
        method: &str,
    ) -> Result<(), String> {
        let record = StateDb::get_invocation_by_uuid(self, invocation_uuid)?
            .ok_or_else(|| format!("Invocation {invocation_uuid} not found"))?;
        StateDb::update_session_capture(self, record.id, session_id, method)
    }

    fn update_resume_acceptance(
        &self,
        invocation_uuid: &str,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String> {
        let record = StateDb::get_invocation_by_uuid(self, invocation_uuid)?
            .ok_or_else(|| format!("Invocation {invocation_uuid} not found"))?;
        StateDb::update_resume_acceptance(self, record.id, status, evidence)
    }

    fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String> {
        StateDb::get_invocation_by_uuid(self, uuid)
    }

    fn list_invocation_children(&self, parent_uuid: &str) -> Result<Vec<InvocationRecord>, String> {
        let parent = StateDb::get_invocation_by_uuid(self, parent_uuid)?
            .ok_or_else(|| format!("Invocation {parent_uuid} not found"))?;
        StateDb::list_invocation_children(self, parent.id)
    }
}

/// Read-only provider facts consumed by routing.
pub trait ProviderRoutingRepository {
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
    ) -> Result<i64, String>;
    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String>;
    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String>;
    fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String>;
}

impl ProviderRoutingRepository for StateDb {
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
    ) -> Result<i64, String> {
        StateDb::recent_error_count(self, model_name, provider_name, window_minutes)
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

/// Provider quota mutation repository.
pub trait ProviderQuotaRepository {
    fn upsert_quota_refresh(
        &self,
        provider_name: &str,
        windows: &[QuotaWindowInput],
    ) -> Result<(), String>;
    fn mark_exhausted(&self, provider_name: &str) -> Result<(), String>;
    fn record_topology_probe(&self, provider_name: &str) -> Result<(), String>;
    fn increment_calls_since_refresh(&self, provider_name: &str) -> Result<(), String>;
}

impl ProviderQuotaRepository for StateDb {
    fn upsert_quota_refresh(
        &self,
        provider_name: &str,
        windows: &[QuotaWindowInput],
    ) -> Result<(), String> {
        StateDb::upsert_quota_refresh(self, provider_name, windows)
    }

    fn mark_exhausted(&self, provider_name: &str) -> Result<(), String> {
        StateDb::mark_exhausted(self, provider_name)
    }

    fn record_topology_probe(&self, provider_name: &str) -> Result<(), String> {
        StateDb::record_topology_probe(self, provider_name)
    }

    fn increment_calls_since_refresh(&self, provider_name: &str) -> Result<(), String> {
        StateDb::increment_calls_since_refresh(self, provider_name)
    }
}

/// Session-turn ingest/read repository.
pub trait SessionTurnRepository {
    fn ingest_session_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
        timestamp: &DateTime<Utc>,
        role: &str,
        source_file: &str,
    ) -> Result<bool, String>;
    fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[SessionTurnIngest],
    ) -> Result<u64, String>;
    fn count_session_turns(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnCounts, String>;
    fn latest_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, DbError>;
    fn flag_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool, DbError>;
}

impl SessionTurnRepository for StateDb {
    fn ingest_session_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
        timestamp: &DateTime<Utc>,
        role: &str,
        source_file: &str,
    ) -> Result<bool, String> {
        StateDb::ingest_session_turn(
            self,
            provider_name,
            session_id,
            turn_id,
            timestamp,
            role,
            source_file,
        )
    }

    fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[SessionTurnIngest],
    ) -> Result<u64, String> {
        StateDb::ingest_session_turns_batch(self, provider_name, turns)
    }

    fn count_session_turns(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnCounts, String> {
        StateDb::count_session_turns(self, provider_name, session_id)
    }

    fn latest_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, DbError> {
        StateDb::latest_compaction_boundary(self, provider_name, session_id)
    }

    fn flag_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool, DbError> {
        StateDb::flag_compaction_boundary(self, provider_name, session_id, turn_id)
    }
}

/// Session-chain and segment repository.
pub trait SessionChainRepository {
    fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError>;
    fn find_conflicting_active_segment(
        &self,
        provider_name: &str,
        session_id: &str,
        own_chain_id: &str,
    ) -> Result<Option<String>, DbError>;
    fn mint_imported_chain_if_absent(
        &self,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<(), DbError>;
    fn close_active_segment_returning(
        &self,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError>;
    fn update_chain_last_used(&self, chain_id: &str) -> Result<(), DbError>;
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
    ) -> Result<Option<i64>, String>;
    fn find_session_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Option<String>, String>;
}

impl SessionChainRepository for StateDb {
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

    fn find_conflicting_active_segment(
        &self,
        provider_name: &str,
        session_id: &str,
        own_chain_id: &str,
    ) -> Result<Option<String>, DbError> {
        StateDb::find_conflicting_active_segment(self, provider_name, session_id, own_chain_id)
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

    fn close_active_segment_returning(
        &self,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        StateDb::close_active_segment_returning(self, chain_id, ended_at)
    }

    fn update_chain_last_used(&self, chain_id: &str) -> Result<(), DbError> {
        StateDb::update_chain_last_used(self, chain_id)
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
    ) -> Result<Option<i64>, String> {
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

/// Resume resolution read repository.
pub trait ResumeRepository {
    fn resolve_resume(
        &self,
        models: &ModelStore,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError>;
    fn resume_previews(&self, input: &str) -> Result<Vec<ChainPreview>, DbError>;
    fn chain_id_for_segment(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError>;
}

impl ResumeRepository for StateDb {
    fn resolve_resume(
        &self,
        models: &ModelStore,
        input: &str,
        model_override: Option<&str>,
    ) -> Result<ResolvedResume, ResumeError> {
        StateDb::resolve_resume(self, models, input, model_override)
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
}

/// Setup/discovery repository used by the GUI setup flow.
pub trait SetupRepository {
    fn upsert_cli_provider(&self, provider: &CliProviderRecord) -> Result<(), String>;
    fn list_cli_providers(&self) -> Result<Vec<CliProviderRecord>, String>;
    fn get_cli_provider(&self, cli_name: &str) -> Result<Option<CliProviderRecord>, String>;
    fn insert_account(&self, account: &AccountRecord) -> Result<(), String>;
    fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String>;
    fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String>;
    fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String>;
    fn list_discovered_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, String>;
    fn delete_stale_models(&self, provider: &str, cli_version: &str) -> Result<u64, String>;
    fn upsert_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        parameter: &ModelParameter,
    ) -> Result<(), String>;
    fn list_model_parameters(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameter>, String>;
}

impl SetupRepository for StateDb {
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

    fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String> {
        StateDb::upsert_discovered_model(self, model)
    }

    fn list_discovered_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, String> {
        StateDb::list_discovered_models(self, provider)
    }

    fn delete_stale_models(&self, provider: &str, cli_version: &str) -> Result<u64, String> {
        StateDb::delete_stale_models(self, provider, cli_version)
    }

    fn upsert_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        parameter: &ModelParameter,
    ) -> Result<(), String> {
        StateDb::upsert_model_parameter(self, model_name, provider, parameter)
    }

    fn list_model_parameters(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameter>, String> {
        StateDb::list_model_parameters(self, model_name, provider)
    }
}

/// Read-only schema compatibility probe over an already-open DB.
pub trait SchemaProbeRepository {
    fn inspect_open_db(&self, path: PathBuf) -> Result<SchemaProbeReport, String>;
}

impl SchemaProbeRepository for StateDb {
    fn inspect_open_db(&self, path: PathBuf) -> Result<SchemaProbeReport, String> {
        let state_db = schema_probe::inspect_schema(self.connection(), path)?;
        Ok(schema_probe::report_from_state_db(state_db))
    }
}

/// State DB opening policy for command composition roots.
pub trait StateDbOpener {
    fn open_default(&self) -> Result<StateDb, String>;
    fn open_at(&self, path: &Path) -> Result<StateDb, String>;
    fn open_in_memory(&self) -> StateDb;
}

#[derive(Debug, Clone, Default)]
pub struct ProductionStateDbOpenerState {
    registration: Option<Arc<DeploymentAwareOpenerRegistration>>,
}

pub type ProductionStateDbOpener = ProductionStateDbOpenerState;

/// Default production opener value, preserving the previous unit-struct construction syntax.
#[allow(non_upper_case_globals)]
pub const ProductionStateDbOpener: ProductionStateDbOpenerState =
    ProductionStateDbOpenerState::new();

impl ProductionStateDbOpenerState {
    pub const fn new() -> Self {
        Self { registration: None }
    }

    pub fn with_deployment_aware_opener(opener: DeploymentAwareOpener) -> Self {
        let token = NEXT_DEPLOYMENT_AWARE_OPENER_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("deployment-aware opener token counter exhausted");
        DEPLOYMENT_AWARE_OPENER.with(|slot| {
            *slot.borrow_mut() = Some(DeploymentAwareOpenerSlot { token, opener });
        });
        Self {
            registration: Some(Arc::new(DeploymentAwareOpenerRegistration { token })),
        }
    }
}

impl Drop for ProductionStateDbOpenerState {
    fn drop(&mut self) {
        let Some(registration) = self.registration.as_ref() else {
            return;
        };

        if Arc::strong_count(registration) > 1 {
            return;
        }

        DEPLOYMENT_AWARE_OPENER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot
                .as_ref()
                .is_some_and(|current| current.token == registration.token)
            {
                *slot = None;
            }
        });
    }
}

impl StateDbOpener for ProductionStateDbOpenerState {
    fn open_default(&self) -> Result<StateDb, String> {
        if let Some(opener) = DEPLOYMENT_AWARE_OPENER
            .with(|slot| slot.borrow().as_ref().map(|slot| slot.opener.clone()))
        {
            opener.open_default().map_err(String::from)
        } else {
            StateDb::open_default()
        }
    }

    fn open_at(&self, path: &Path) -> Result<StateDb, String> {
        StateDb::open(path)
    }

    fn open_in_memory(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).expect("in-memory StateDb must open")
    }
}
