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
    let state = services
        .state_db_opener
        .open_default()
        .map_err(|err| format!("failed to open state db: {err}"))?;
    let accounts = accessor::collect_accounts(providers, models)?;
    let filtered = filter::filter_routing_pool(accounts);
    let outcomes = fetcher::fetch_all(&state, &filtered);
    let rows = mapper::map_rows(outcomes, &filtered);
    renderer::render(&rows, writer)
        .map_err(|err| format!("failed to render usage table: {err}"))?;
    Ok(0)
}
