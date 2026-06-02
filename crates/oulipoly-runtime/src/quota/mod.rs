//! Per-provider quota refresh. Runs a user-supplied script (from
//! `providers.toml`) that hits the provider's usage API and prints JSON on
//! stdout. The parsed reading lands in `provider_quotas` + `provider_quota_windows`.

mod adapter_derived_source;
mod external_provider;
mod freshness;
mod in_flight;
pub mod marker_verification;
mod outcome;
mod parse;
mod process;
mod refresh;
mod source;

pub use freshness::{
    TOPOLOGY_PROBE_COOLDOWN_SECS, dynamic_ttl_secs, is_routing_stale, is_stale,
    is_topology_probe_due,
};
pub use in_flight::{InFlight, InFlightGuard};
pub use marker_verification::verify_or_clear_marker;
pub use outcome::{QuotaScriptWindow, RefreshOutcome};
pub use parse::parse_output;
pub use process::{run_refresh_command, run_script};
pub use refresh::{RuntimeQuotaService, refresh_provider, refresh_provider_for_routing};
pub use source::has_refresh_source;
