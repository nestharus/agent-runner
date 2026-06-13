//! ## Declared roles
//!
//! - accessor
//! - mapper
//!
//! Role set: { accessor, mapper }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/setup_helpers.rs
//!     role: intrinsic-surface
//!     Domain: setup-helpers-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::super::*;
pub(in crate::db::tests) fn sample_provider() -> CliProviderRecord {
    CliProviderRecord {
        cli_name: "provider-a".to_string(),
        display_name: "Anthropic".to_string(),
        installed: true,
        version: Some("1.2.3".to_string()),
        config_dir: Some("/home/user/.provider-a".to_string()),
        last_synced: None,
    }
}

pub(in crate::db::tests) fn sample_discovered_model(name: &str, provider: &str) -> DiscoveredModel {
    DiscoveredModel {
        canonical_name: name.to_string(),
        provider: provider.to_string(),
        discovered_at: "2026-02-19T00:00:00Z".to_string(),
        cli_version: "1.0.0".to_string(),
    }
}
