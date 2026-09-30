//! Host PID namespace classifier, durable registries, and fail-closed host
//! entry staging. The installed service remains opt-in; connected control E
//! does not by itself establish terminal or physical-drain completion.
#![cfg(target_os = "linux")]

pub mod accepted_grant;
pub mod admission_accounting;
pub mod codex_raw_verifier;
pub mod connected_control;
pub mod cutover_gate;
pub mod entry_registry;
pub mod first_install_activation;
pub mod identity;
pub mod installed_launch;
pub mod installed_launch_ledger;
pub mod installed_pair;
pub mod json_artifact;
pub mod native_receipt;
pub mod normal_model_selection;
pub mod normal_physical;
pub mod normal_plan_custody;
pub mod protocol;
pub mod registry;
pub mod root_drain;
pub mod root_pid1;
pub mod source_acceptance;
pub mod source_candidate;
pub mod source_physical;
pub mod successor_launch;
pub mod work_registry;
pub mod writer_census;

pub use identity::{Classification, PeerIdentity, PinnedProcess, classify_peer};
pub use registry::{RootRecord, RootRegistry};
pub use work_registry::{Scope, WorkRecord, WorkRegistry, classify_scope, classify_scope_readback};
