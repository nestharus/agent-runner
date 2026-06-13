//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - orchestration
//! - validator
//!
//! Role set: { accessor, mapper, orchestration, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/age160_lifecycle_helpers.rs
//!     role: intrinsic-surface
//!     Domain: age160-lifecycle-helpers-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::super::*;
#[derive(Clone, Default)]
pub(in crate::db::tests) struct Age160LifecycleSink {
    pub(in crate::db::tests) records: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl LifecycleEventSink for Age160LifecycleSink {
    fn forward(&mut self, record: &serde_json::Value) {
        self.records.lock().unwrap().push(record.clone());
    }
}

pub(in crate::db::tests) fn age160_lifecycle_fixture() -> (
    StateDb,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Age160LifecycleSink {
        records: records.clone(),
    };
    let db = StateDb::open_with_sink(Path::new(":memory:"), Box::new(sink)).unwrap();
    (db, records)
}

pub(in crate::db::tests) fn age160_invocation_start(uuid: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: uuid.to_string(),
        model_name: "provider-b~high".to_string(),
        provider_name: "provider-b2".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

pub(in crate::db::tests) fn age160_lifecycle_records(
    records: &std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) -> Vec<serde_json::Value> {
    records.lock().unwrap().clone()
}

pub(in crate::db::tests) fn age160_record_keys(record: &serde_json::Value) -> Vec<&str> {
    map_age160_record_keys(require_age160_record_object(record))
}

fn require_age160_record_object(
    record: &serde_json::Value,
) -> &serde_json::Map<String, serde_json::Value> {
    record.as_object().expect("record object")
}

fn map_age160_record_keys(record: &serde_json::Map<String, serde_json::Value>) -> Vec<&str> {
    record.keys().map(String::as_str).collect()
}

/// AGE-160 risk: A6 db.rs↔lifecycle_log facade narrowing.
/// Selected level: unit + integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A6.
pub(in crate::db::tests) fn age160_direct_symbol_count(haystack: &str, needles: &[&str]) -> usize {
    needles
        .iter()
        .map(|needle| haystack.match_indices(needle).count())
        .sum()
}
