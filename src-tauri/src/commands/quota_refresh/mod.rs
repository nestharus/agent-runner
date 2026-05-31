//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/mod.rs
//!     role: adapter
//!     Translates:
//!       - QuotaRefreshEntry serialization contract
//!       - QuotaRefreshWindow serialization contract
//!       - frontend quota-refresh DTO compatibility contract
//! ```

mod accessor;
mod candidates;
mod identity;
mod mapper;
pub mod orchestration;

use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct QuotaRefreshWindow {
    pub used_percent: f64,
    pub resets_at: String,
}

#[derive(Serialize, Debug)]
pub struct QuotaRefreshEntry {
    pub provider_name: String,
    pub status: String,
    pub windows: Vec<QuotaRefreshWindow>,
    pub message: Option<String>,
}

pub use orchestration::refresh_quotas_inner;
pub(crate) use orchestration::{__cmd__refresh_quotas, refresh_quotas};
