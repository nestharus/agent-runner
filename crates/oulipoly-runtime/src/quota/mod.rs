//! Per-provider quota refresh. Runs a user-supplied script (from
//! `providers.toml`) that hits the provider's usage API and prints JSON on
//! stdout. The parsed reading lands in `provider_quotas` + `provider_quota_windows`.
//!
//! ## Declared roles
//! accessor
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/mod.rs
//!     role: intrinsic-surface
//!     Domain: quota_module_facade
//!     Owns:
//!       - quota submodule declarations and the quota public re-export surface
//! ```

mod adapter_derived_source;
mod auth_refresh_lock;
mod external_provider;
mod freshness;
mod in_flight;
mod lock_paths;
pub mod marker_verification;
mod outcome;
mod parse;
mod process;
mod refresh;
mod source;

pub use auth_refresh_lock::{AuthRefreshAttempt, run_auth_refresh_command_coalesced};
pub use freshness::{
    TOPOLOGY_PROBE_COOLDOWN_SECS, dynamic_ttl_secs, is_routing_stale, is_stale,
    is_topology_probe_due,
};
pub use in_flight::{InFlight, InFlightGuard};
pub use lock_paths::{data_home as lock_data_home, sanitize_lock_name as sanitize_lock_key};
pub use marker_verification::verify_or_clear_marker;
pub use outcome::{QuotaScriptWindow, RefreshOutcome};
pub use parse::parse_output;
pub use process::{run_refresh_command, run_script};
pub use refresh::{RuntimeQuotaService, refresh_provider, refresh_provider_for_routing};
pub use source::has_refresh_source;
