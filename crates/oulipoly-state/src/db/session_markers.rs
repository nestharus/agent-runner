//! ## Declared roles
//!
//! - formatter
//!
//! Role set: { formatter }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/session_markers.rs
//!     role: intrinsic-surface
//!     Domain: session-markers-persistence
//!     Owns:
//!       - the StateDb session-markers surface this concern owns, split from the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - all StateDb/rusqlite carriers and concern-owned DTOs/macros referenced
//!         via `use super::*`, subordinate to this domain
//! ```

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionMarkerPayload {
    pub agent_runner_chain_id: Option<String>,
    pub agent_runner_invocation_id: String,
    #[serde(rename = "id")]
    pub legacy_id: String,
    pub provider_name: Option<String>,
    pub provider_session_id: Option<String>,
    pub resume_input_id: Option<String>,
    #[serde(rename = "session_id")]
    pub legacy_session_id: Option<String>,
}

impl SessionMarkerPayload {
    pub fn stderr_line(&self) -> String {
        let payload = serde_json::to_string(self).expect("SessionMarkerPayload serializes");
        format!("OULIPOLY_SESSION={payload}\n")
    }
}
