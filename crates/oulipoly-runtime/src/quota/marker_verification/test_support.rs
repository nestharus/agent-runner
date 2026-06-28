//! Test-only helpers exposed so other in-crate tests can borrow the same
//! env-mutation guard.
//!
//! ## Declared roles
//! orchestration, accessor, mapper
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/marker_verification/test_support.rs
//!     role: adapter
//!     Translates:
//!       - Rust process environment mutation contract (`std::env::var_os`, `std::env::set_var`, `std::env::remove_var`)
//!       - Rust test synchronization contract (`Mutex`, `MutexGuard`, `OnceLock`)
//! ```

use crate::test_support::lock_env;
use std::ffi::{OsStr, OsString};
use std::sync::MutexGuard;

type EnvMutation = (&'static str, Option<OsString>);
type EnvSnapshot = (&'static str, Option<OsString>);

/// Scoped guard that sets env vars on construction and restores the prior
/// values (or removes them) on drop.
pub(crate) struct EnvGuard {
    vars: Vec<EnvSnapshot>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Acquire the global env mutex, snapshot the previous value, and set
    /// `name` to `value` for the lifetime of the returned guard.
    pub(crate) fn set(name: &'static str, value: impl AsRef<OsStr>) -> Self {
        Self::set_many(vec![env_set_mutation(name, value.as_ref())])
    }

    /// Acquire the global env mutex, snapshot the previous value, and remove
    /// `name` for the lifetime of the returned guard.
    pub(crate) fn remove(name: &'static str) -> Self {
        Self::set_many(vec![env_remove_mutation(name)])
    }

    /// Acquire the global env mutex once for a batch of env mutations.
    pub(crate) fn set_many(vars: Vec<EnvMutation>) -> Self {
        let lock = acquire_env_lock();
        let previous = previous_env_values(&vars);
        apply_env_mutations(vars);
        Self {
            vars: previous,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore_env_values(&mut self.vars);
    }
}

fn env_set_mutation(name: &'static str, value: &OsStr) -> EnvMutation {
    (name, Some(value.to_os_string()))
}

fn env_remove_mutation(name: &'static str) -> EnvMutation {
    (name, None)
}

fn acquire_env_lock() -> MutexGuard<'static, ()> {
    lock_env()
}

fn previous_env_values(vars: &[EnvMutation]) -> Vec<EnvSnapshot> {
    vars.iter().map(previous_env_value).collect()
}

fn previous_env_value(mutation: &EnvMutation) -> EnvSnapshot {
    (mutation.0, std::env::var_os(mutation.0))
}

fn apply_env_mutations(vars: Vec<EnvMutation>) {
    for mutation in vars {
        apply_env_mutation(mutation);
    }
}

fn apply_env_mutation(mutation: EnvMutation) {
    // Safety: the global env lock serializes set/remove across all tests that
    // route through this helper, so the guard owns these variables.
    unsafe {
        match mutation {
            (name, Some(value)) => std::env::set_var(name, value),
            (name, None) => std::env::remove_var(name),
        }
    }
}

fn restore_env_values(vars: &mut Vec<EnvSnapshot>) {
    for snapshot in vars.drain(..).rev() {
        restore_env_value(snapshot);
    }
}

fn restore_env_value(snapshot: EnvSnapshot) {
    // Safety: the EnvGuard still holds `_lock` until after this restore.
    unsafe {
        match snapshot {
            (name, Some(prev)) => std::env::set_var(name, prev),
            (name, None) => std::env::remove_var(name),
        }
    }
}
