//! ## Declared roles
//!
//! Roles: synchronizer.
//!
//! TEST: process-wide synchronization for proactive wake integration cases.

use std::sync::{Mutex, MutexGuard, OnceLock};

pub(crate) fn integration_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn integration_test_guard() -> MutexGuard<'static, ()> {
    integration_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
