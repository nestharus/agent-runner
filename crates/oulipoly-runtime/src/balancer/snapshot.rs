use oulipoly_config::ModelConfig;
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};

pub(super) struct QuotaSnapshot {
    pub(super) quotas: Vec<Option<QuotaRecord>>,
    pub(super) windows: Vec<Vec<QuotaWindow>>,
}

pub(super) fn load_quota_snapshot(model: &ModelConfig, state: &StateDb) -> QuotaSnapshot {
    QuotaSnapshot {
        quotas: model
            .providers
            .iter()
            .map(|provider| cached_quota_record(state, &provider.name))
            .collect(),
        windows: model
            .providers
            .iter()
            .map(|provider| cached_quota_windows(state, &provider.name))
            .collect(),
    }
}

pub(super) fn cached_quota_record(state: &StateDb, provider_name: &str) -> Option<QuotaRecord> {
    state.get_quota(provider_name).ok().flatten()
}

pub(super) fn cached_quota_windows(state: &StateDb, provider_name: &str) -> Vec<QuotaWindow> {
    state.get_windows(provider_name).unwrap_or_default()
}
