//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/lib.rs
//!     role: intrinsic-surface
//!     Domain: oulipoly_runtime_module_facade
//!     Owns:
//!       - runtime child module declarations and root-public module exposure
//! ```
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/lib.rs::test_support
//!     role: adapter
//!     Translates:
//!       - process-environment test serialization contract
//!       - std::sync mutex initialization and guard contract
//! ```

pub mod balancer;
pub mod delivery_evidence;
pub mod diagnostics;
pub mod discovery;
pub mod executor;
pub mod fresh_continuation;
pub mod migration;
pub mod observability;
pub mod ports;
pub mod provider_registry;
pub mod provider_settings;
pub mod quota;
pub mod repl_default_provider;
pub mod rotation_domain;
pub mod rotation_external_provider;
pub mod rotation_host_apply;
pub mod rotation_journal;
pub mod services;
pub mod session_export;
pub mod session_external_provider;
pub mod session_ingress;
pub mod session_lock;
pub mod session_metadata;
pub mod session_provider;
pub mod session_replace;
pub mod session_supervisor;
pub mod sessions;
pub mod trace;

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub(crate) fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    pub(crate) fn lock_env() -> MutexGuard<'static, ()> {
        env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
