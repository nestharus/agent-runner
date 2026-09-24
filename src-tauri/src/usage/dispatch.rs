use crate::usage::row::RowState;
use crate::usage::{accessor, fetcher, filter, mapper, renderer};
use crate::wiring::AgentRuntimeServices;
use oulipoly_config::{ModelConfig, ProvidersConfig};
use oulipoly_state::repositories::StateDbOpener;
use std::io::Write;

pub(crate) fn run_usage(
    services: &AgentRuntimeServices,
    providers: &ProvidersConfig,
    models: &[ModelConfig],
    writer: &mut impl Write,
) -> Result<i32, String> {
    #[cfg(feature = "age319-private-broker-fixture")]
    if std::env::var_os("AGE319_PRIVATE_FRESH_PROVIDER_V1").is_some() {
        return Err(
            "private fresh quota Q manual refresh is unavailable; no broker quota probe was started"
                .into(),
        );
    }
    let state = services
        .state_db_opener
        .open_default()
        .map_err(|err| format!("failed to open state db: {err}"))?;
    let accessor::CollectAccountsOutput { accounts, warnings } =
        accessor::collect_accounts(providers, models);
    for warning in &warnings {
        eprintln!("{warning}");
    }
    let filtered = filter::filter_routing_pool(accounts);
    let outcomes = fetcher::fetch_all(&state, &filtered);
    let rows = mapper::map_rows(outcomes, &filtered);
    renderer::render(&rows, writer)
        .map_err(|err| format!("failed to render usage table: {err}"))?;
    // A rendered row is not proof that a fresh quota observation was obtained.
    // In particular, failed scripts and in-flight probes must not make a
    // manual refresh look successful to callers that check the process status.
    let incomplete = !warnings.is_empty()
        || rows.iter().any(|row| {
            matches!(
                row.row_state,
                RowState::Error(_) | RowState::InFlight | RowState::NoWindows
            ) || row.cache_warning.is_some()
        });
    #[cfg(feature = "age319-private-broker-fixture")]
    eprintln!(
        "--usage refreshed only the legacy StateDb; private broker quota Q was not refreshed"
    );
    Ok(i32::from(incomplete))
}
