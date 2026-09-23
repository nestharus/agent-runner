//! Host PID namespace classifier and durable root registry. This crate has no
//! hooks into Runner admission; the service must be installed and started
//! explicitly after the paired integration is complete.
#![cfg(target_os = "linux")]

pub mod identity;
pub mod protocol;
pub mod registry;

pub use identity::{Classification, PeerIdentity, PinnedProcess, classify_peer};
pub use registry::{RootRecord, RootRegistry};
