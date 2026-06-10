//! ## Declared roles
//!
//! `accessor`.
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/context.rs::balance_context_surface
//!     role: intrinsic-surface
//!     Domain: contextual-balancer-dependencies
//!     Owns:
//!       - ProvidersConfig quota refresh-source contract
//!       - SessionsConfig session scan and adapter-derived refresh contract
//!       - InFlight refresh deduplication carrier

use crate::quota::InFlight;
use oulipoly_config::{ProvidersConfig, SessionsConfig};

/// Contextual dependencies for quota-aware balancing. When present,
/// `select_provider` will trigger a synchronous refresh for any provider
/// whose cached quota is stale (older than `REFRESH_TTL_HOURS`) AND scan
/// each provider's CLI session logs for new turns. Pass `None` to use
/// cached-only scoring (e.g. from inside an async handler where blocking
/// on a network call isn't desirable).
pub struct BalanceContext<'a> {
    pub providers_cfg: &'a ProvidersConfig,
    pub sessions_cfg: &'a SessionsConfig,
    pub in_flight: &'a InFlight,
}
