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
//!       - InFlight refresh deduplication carrier

use crate::quota::InFlight;
use oulipoly_config::ProvidersConfig;

/// Contextual dependencies for quota-aware balancing. When present,
/// `select_provider` will trigger a synchronous quota refresh for a provider
/// whose cached quota is stale. Session-turn ingestion remains an independent
/// bounded background stream. Pass `None` to use cached-only scoring.
pub struct BalanceContext<'a> {
    pub providers_cfg: &'a ProvidersConfig,
    pub in_flight: &'a InFlight,
}
