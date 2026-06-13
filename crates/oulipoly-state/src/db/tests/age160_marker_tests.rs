//! ## Declared roles
//!
//! - validator
//! - parser
//! - formatter
//! - mapper
//!
//! Role set: { validator, parser, formatter, mapper }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/age160_marker_tests.rs
//!     role: intrinsic-surface
//!     Domain: age160-marker-tests-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::*;
#[test]
fn age160_composite_invocation_id_formats_canonical_marker() {
    let composite = CompositeInvocationId {
        source: "provider-b2".to_string(),
        id: known_marker_uuid().to_string(),
    };

    let stderr_line = composite.stderr_line();
    assert!(stderr_line.starts_with("OULIPOLY_INVOCATION="));
    let payload = stderr_line
        .strip_prefix("OULIPOLY_INVOCATION=")
        .expect("stderr marker prefix");
    assert!(!payload.starts_with("OULIPOLY_INVOCATION="));
    assert_eq!(
        payload,
        r#"{"source":"provider-b2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );
}

#[test]
fn age160_composite_invocation_id_parses_canonical_payload() {
    let known_uuid = known_marker_uuid();
    let payload = r#"{"source":"provider-b2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#;

    let parsed = CompositeInvocationId::parse_env_value(payload).unwrap();
    assert_eq!(parsed.source, "provider-b2");
    assert_eq!(parsed.id.to_string(), known_uuid.to_string());

    let parent_env = parent_env_payload(&CompositeInvocationId {
        source: "provider-b2".to_string(),
        id: known_uuid.to_string(),
    });
    assert!(!parent_env.starts_with("OULIPOLY_INVOCATION="));
    assert_eq!(
        CompositeInvocationId::parse_env_value(&parent_env)
            .unwrap()
            .id
            .to_string(),
        known_uuid.to_string()
    );
}

#[test]
fn age160_composite_invocation_id_declared_grammar_legacy_shell_mangled_compatibility() {
    let known_uuid = "7ad2916c-38dd-49e6-a1f7-3ef22766ff70";

    for payload in legacy_marker_payloads(known_uuid) {
        assert!(
            !payload.starts_with("OULIPOLY_INVOCATION="),
            "legacy compatibility payloads are raw payloads, not marker lines"
        );
        let parsed = CompositeInvocationId::parse_env_value(&payload).unwrap();
        assert_eq!(parsed.source, "provider-b2");
        assert_eq!(parsed.id.to_string(), known_uuid);
    }

    assert!(
        CompositeInvocationId::parse_env_value("{source:'provider-b2',id:'not-a-uuid'}").is_err()
    );
}

fn known_marker_uuid() -> Uuid {
    Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap()
}

fn parent_env_payload(composite: &CompositeInvocationId) -> String {
    serde_json::to_string(composite).unwrap()
}

fn legacy_marker_payloads(known_uuid: &str) -> [String; 2] {
    [
        double_quoted_legacy_marker_payload(known_uuid),
        single_quoted_legacy_marker_payload(known_uuid),
    ]
}

fn double_quoted_legacy_marker_payload(known_uuid: &str) -> String {
    format!("{{source:\"provider-b2\",id:\"{known_uuid}\",extra:\"ignored\"}}")
}

fn single_quoted_legacy_marker_payload(known_uuid: &str) -> String {
    format!("{{source:'provider-b2',id:'{known_uuid}',extra:'ignored'}}")
}
