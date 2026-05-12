use oulipoly_config::ProviderEntry;

#[derive(Clone, Debug)]
pub(crate) struct EnumeratedAccount {
    pub account_id: String,
    pub vendor: String,
    pub provider_entry: ProviderEntry,
}

#[derive(Clone, Debug)]
pub(crate) struct UsageRow {
    pub account_id: String,
    pub vendor: String,
    pub windows: Vec<UsageWindow>,
    pub row_state: RowState,
}

#[derive(Clone, Debug)]
pub(crate) enum RowState {
    HasWindows,
    NoUsageApi,
    Error(String),
    InFlight,
    NoWindows,
}

#[derive(Clone, Debug)]
pub(crate) struct UsageWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: String,
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    pub unit: Option<String>,
}
