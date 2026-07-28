//! ## Declared roles
//!
//! `filter`, `mapper`.
//!
//! ## Component declared roles
//! ```yaml
//! component_declared_roles:
//!   - component: scratchpad-retirement-status
//!     paths:
//!       - crates/oulipoly-agent-scratchpad/src/retirement_status.rs
//!       - crates/oulipoly-agent-scratchpad/src/retirement_status/tests.rs
//!     roles:
//!       - filter
//!       - mapper
//! ```

use chrono::{DateTime, Utc};

use super::ScratchpadAddress;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RetirementStatus {
    Retired,
    AlreadyRetired,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct DeleteStatusReduction {
    pub(super) tombstoned_versions: Vec<u64>,
    pub(super) already_tombstoned_versions: Vec<u64>,
    pub(super) last_tombstoned_at: Option<DateTime<Utc>>,
}

pub(super) fn partition_delete_version(
    reduction: &mut DeleteStatusReduction,
    version: u64,
    status: RetirementStatus,
) {
    match status {
        RetirementStatus::Retired => reduction.tombstoned_versions.push(version),
        RetirementStatus::AlreadyRetired => reduction.already_tombstoned_versions.push(version),
    }
}

pub(super) fn project_last_delete_tombstoned_at(
    reduction: &mut DeleteStatusReduction,
    tombstoned_at: DateTime<Utc>,
) {
    reduction.last_tombstoned_at = Some(tombstoned_at);
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct GcStatusReduction {
    pub(super) tombstoned_rows: Vec<ScratchpadAddress>,
    pub(super) already_tombstoned_rows: Vec<ScratchpadAddress>,
}

pub(super) fn partition_gc_address(
    reduction: &mut GcStatusReduction,
    address: ScratchpadAddress,
    status: RetirementStatus,
) {
    match status {
        RetirementStatus::Retired => reduction.tombstoned_rows.push(address),
        RetirementStatus::AlreadyRetired => reduction.already_tombstoned_rows.push(address),
    }
}

pub(super) fn map_gc_dry_run_addresses(addresses: Vec<ScratchpadAddress>) -> GcStatusReduction {
    GcStatusReduction {
        tombstoned_rows: addresses,
        already_tombstoned_rows: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
