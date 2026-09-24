//! Host PID namespace classifier, durable registries, and fail-closed host
//! entry staging. The service is uninstalled; child and work handoff remain
//! unavailable pending paired integration.
#![cfg(target_os = "linux")]

pub mod accepted_grant;
pub mod cutover_gate;
pub mod entry_registry;
pub mod identity;
pub mod installed_launch;
pub mod installed_pair;
pub mod native_receipt;
pub mod protocol;
pub mod registry;
pub mod work_registry;
pub mod writer_census;

pub use identity::{Classification, PeerIdentity, PinnedProcess, classify_peer};
pub use registry::{RootRecord, RootRegistry};
pub use work_registry::{Scope, WorkRecord, WorkRegistry, classify_scope};
