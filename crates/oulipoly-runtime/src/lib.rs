pub mod balancer;
pub mod diagnostics;
pub mod discovery;
pub mod executor;
pub mod migration;
pub mod ports;
pub mod provider_registry;
pub mod provider_settings;
pub mod quota;
pub mod repl_default_provider;
pub mod services;
pub mod session_export;
pub mod session_lock;
pub mod session_metadata;
pub mod session_provider;
pub mod session_replace;
pub mod sessions;
pub mod trace;

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, OnceLock};

    pub(crate) fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}
