pub mod balancer;
pub mod diagnostics;
pub mod discovery;
pub mod executor;
pub mod migration;
pub mod quota;
pub mod session_export;
pub mod session_lock;
pub mod session_metadata;
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
