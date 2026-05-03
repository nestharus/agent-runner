#![allow(dead_code)]

use agent_runner_lib::balancer::BalanceEffects;
use agent_runner_lib::runtime::RuntimePaths;
use agent_runner_lib::state::{ProviderRecord, QuotaRecord, QuotaWindow, RoutingRepository};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct CallLog {
    events: Arc<Mutex<Vec<String>>>,
}

impl CallLog {
    pub fn push(&self, event: impl Into<String>) {
        self.events.lock().unwrap().push(event.into());
    }

    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    pub fn assert_prefix(&self, expected: &[&str]) {
        let actual = self.events();
        let prefix = actual
            .iter()
            .take(expected.len())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            prefix,
            expected
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
            "full call log: {actual:?}"
        );
    }
}

#[derive(Clone)]
pub struct FixtureRuntimePaths {
    data_root: PathBuf,
    config_root: PathBuf,
    models_dir: PathBuf,
    agents_dir: PathBuf,
    state_db_path: PathBuf,
    providers_path: PathBuf,
    sessions_path: PathBuf,
    lock_dir: PathBuf,
    replace_journal_dir: PathBuf,
}

impl FixtureRuntimePaths {
    pub fn new(root: &Path) -> Self {
        let config_root = root.join("config").join("oulipoly-agent-runner");
        let data_root = root.join("data").join("oulipoly-agent-runner");
        Self {
            models_dir: config_root.join("models"),
            agents_dir: config_root.join("agents"),
            providers_path: config_root.join("providers.toml"),
            sessions_path: config_root.join("sessions.toml"),
            state_db_path: data_root.join("state.db"),
            lock_dir: data_root.join("locks"),
            replace_journal_dir: data_root.join("replace_journal"),
            data_root,
            config_root,
        }
    }

    pub fn with_existing_paths(
        data_root: PathBuf,
        config_root: PathBuf,
        models_dir: PathBuf,
        providers_path: PathBuf,
        sessions_path: PathBuf,
        state_db_path: PathBuf,
    ) -> Self {
        Self {
            agents_dir: config_root.join("agents"),
            lock_dir: data_root.join("locks"),
            replace_journal_dir: data_root.join("replace_journal"),
            data_root,
            config_root,
            models_dir,
            providers_path,
            sessions_path,
            state_db_path,
        }
    }
}

impl RuntimePaths for FixtureRuntimePaths {
    fn data_root(&self) -> Result<PathBuf, String> {
        Ok(self.data_root.clone())
    }

    fn config_root(&self) -> PathBuf {
        self.config_root.clone()
    }

    fn models_dir(&self) -> PathBuf {
        self.models_dir.clone()
    }

    fn agents_dir(&self) -> PathBuf {
        self.agents_dir.clone()
    }

    fn state_db_path(&self) -> Result<PathBuf, String> {
        Ok(self.state_db_path.clone())
    }

    fn providers_path(&self) -> PathBuf {
        self.providers_path.clone()
    }

    fn sessions_path(&self) -> PathBuf {
        self.sessions_path.clone()
    }

    fn lock_dir(&self) -> Result<PathBuf, String> {
        Ok(self.lock_dir.clone())
    }

    fn replace_journal_dir(&self) -> Result<PathBuf, String> {
        Ok(self.replace_journal_dir.clone())
    }
}

pub struct FakeBalanceEffects {
    log: CallLog,
}

impl FakeBalanceEffects {
    pub fn new(log: CallLog) -> Self {
        Self { log }
    }
}

impl BalanceEffects for FakeBalanceEffects {
    fn refresh_quota_if_stale(&self, provider_name: &str) {
        self.log.push(format!("effect:refresh:{provider_name}"));
    }

    fn scan_provider_sessions(&self, provider_name: &str) {
        self.log.push(format!("effect:scan:{provider_name}"));
    }
}

pub struct LoggingRoutingRepository {
    log: CallLog,
}

impl LoggingRoutingRepository {
    pub fn new(log: CallLog) -> Self {
        Self { log }
    }
}

impl RoutingRepository for LoggingRoutingRepository {
    fn get_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<Option<ProviderRecord>, String> {
        self.log
            .push(format!("repo:get_provider:{model_name}:{provider_name}"));
        Ok(None)
    }

    fn recent_error_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<u64, String> {
        self.log.push(format!(
            "repo:recent_error_count:{model_name}:{provider_name}:{window_minutes}"
        ));
        Ok(0)
    }

    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        self.log.push(format!("repo:get_quota:{provider_name}"));
        Ok(None)
    }

    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        self.log.push(format!("repo:get_windows:{provider_name}"));
        Ok(Vec::new())
    }

    fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        self.log.push(format!(
            "repo:count_assistant_turns_since:{provider_name}:{}",
            since.is_some()
        ));
        Ok(0)
    }
}

pub struct GetQuotaMustNotBeCalled {
    log: CallLog,
}

impl GetQuotaMustNotBeCalled {
    pub fn new(log: CallLog) -> Self {
        Self { log }
    }
}

impl RoutingRepository for GetQuotaMustNotBeCalled {
    fn get_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<Option<ProviderRecord>, String> {
        self.log
            .push(format!("repo:get_provider:{model_name}:{provider_name}"));
        Ok(None)
    }

    fn recent_error_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<u64, String> {
        self.log.push(format!(
            "repo:recent_error_count:{model_name}:{provider_name}:{window_minutes}"
        ));
        Ok(0)
    }

    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        panic!("manual target validation must run before get_quota({provider_name})");
    }

    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        self.log.push(format!("repo:get_windows:{provider_name}"));
        Ok(Vec::new())
    }

    fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        self.log.push(format!(
            "repo:count_assistant_turns_since:{provider_name}:{}",
            since.is_some()
        ));
        Ok(0)
    }
}
