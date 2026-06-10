//! ## Declared roles
//!
//! `orchestration`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/tests/mod.rs::balancer_test_harness_adapter
//!     role: adapter
//!     Translates:
//!       - balancer test module inventory into shared config and support fixtures
//! ```

use super::*;
use crate::quota::InFlight;
use chrono::{Duration, SecondsFormat, Utc};
use oulipoly_config::{
    ProviderConfig, ProviderEntry, ProvidersConfig, SessionSourceEntry, SessionsConfig,
    model::PromptMode,
};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use uuid::Uuid;

mod support;
use support::*;

mod basic;
mod burn_rate;
mod density_core;
mod density_fanout;
mod density_helpers;
mod exhaustion;
mod invocation_fallback;
mod live_load;
mod migration_nonalpha;
mod migration_scoring;
mod placement_pin;
mod projections;
mod refresh_topology;
mod source_guard;
mod source_text;
mod topology_refresh;
