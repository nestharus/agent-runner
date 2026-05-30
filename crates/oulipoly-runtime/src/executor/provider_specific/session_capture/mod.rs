//! ## Declared roles
//!
//! Roles: none.
//!
//! Functionless module inventory; no A1 function-role claim.

mod telemetry_scrub;

pub(in crate::executor) use telemetry_scrub::remove_unsanctioned_money_fields;
