use crate::usage::fetcher::QuotaScriptOutcome;
use crate::usage::row::{EnumeratedAccount, RowState, UsageRow, UsageWindow};
use oulipoly_runtime::quota::QuotaScriptWindow;
use std::collections::HashMap;

pub(crate) fn map_rows(
    outcomes: Vec<(String, QuotaScriptOutcome)>,
    accounts: &[EnumeratedAccount],
) -> Vec<UsageRow> {
    let metadata: HashMap<&str, &EnumeratedAccount> = accounts
        .iter()
        .map(|account| (account.account_id.as_str(), account))
        .collect();
    outcomes
        .into_iter()
        .filter_map(|(account_id, outcome)| {
            let account = metadata.get(account_id.as_str())?;
            Some(map_row(account, outcome))
        })
        .collect()
}

fn map_row(account: &EnumeratedAccount, outcome: QuotaScriptOutcome) -> UsageRow {
    let (row_state, windows, cache_warning) = match outcome {
        QuotaScriptOutcome::Updated {
            script_windows,
            cache_warning,
        } => (
            window_state(&script_windows),
            script_windows
                .into_iter()
                .enumerate()
                .map(map_window)
                .collect(),
            cache_warning,
        ),
        QuotaScriptOutcome::NoScript => (RowState::NoUsageApi, Vec::new(), None),
        QuotaScriptOutcome::AlreadyInFlight => (RowState::InFlight, Vec::new(), None),
        QuotaScriptOutcome::Failed(message) => (RowState::Error(message), Vec::new(), None),
    };
    UsageRow {
        account_id: account.account_id.clone(),
        vendor: account.vendor.clone(),
        windows,
        row_state,
        cache_warning,
    }
}

fn window_state(windows: &[QuotaScriptWindow]) -> RowState {
    if windows.is_empty() {
        RowState::NoWindows
    } else {
        RowState::HasWindows
    }
}

fn map_window((index, window): (usize, QuotaScriptWindow)) -> UsageWindow {
    UsageWindow {
        label: window.label.unwrap_or_else(|| format!("window-{index}")),
        used_percent: window.used_percent / 100.0,
        resets_at: window.resets_at,
        limit: window.limit,
        remaining: window.remaining,
        unit: window.unit,
    }
}
