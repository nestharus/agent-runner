use crate::usage::fetcher::QuotaScriptOutcome;
use crate::usage::row::{EnumeratedAccount, RowState, UsageRow, UsageWindow};
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
            let (row_state, windows) = match outcome {
                QuotaScriptOutcome::Updated { script_windows } if script_windows.is_empty() => {
                    (RowState::NoWindows, Vec::new())
                }
                QuotaScriptOutcome::Updated { script_windows } => {
                    let windows = script_windows
                        .into_iter()
                        .enumerate()
                        .map(|(index, window)| UsageWindow {
                            label: window.label.unwrap_or_else(|| format!("window-{index}")),
                            used_percent: window.used_percent / 100.0,
                            resets_at: window.resets_at,
                            limit: window.limit,
                            remaining: window.remaining,
                            unit: window.unit,
                        })
                        .collect();
                    (RowState::HasWindows, windows)
                }
                QuotaScriptOutcome::NoScript => (RowState::NoUsageApi, Vec::new()),
                QuotaScriptOutcome::AlreadyInFlight => (RowState::InFlight, Vec::new()),
                QuotaScriptOutcome::Failed(message) => (RowState::Error(message), Vec::new()),
            };
            Some(UsageRow {
                account_id,
                vendor: account.vendor.clone(),
                windows,
                row_state,
            })
        })
        .collect()
}
