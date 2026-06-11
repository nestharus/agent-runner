//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::*;
#[test]
fn age160_composite_invocation_id_declared_grammar_canonical_json_round_trip() {
    let known_uuid = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let composite = CompositeInvocationId {
        source: "provider-b2".to_string(),
        id: known_uuid.to_string(),
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

    let parsed = CompositeInvocationId::parse_env_value(payload).unwrap();
    assert_eq!(parsed.source, "provider-b2");
    assert_eq!(parsed.id.to_string(), known_uuid.to_string());

    let parent_env = serde_json::to_string(&composite).unwrap();
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

    for payload in [
        format!("{{source:\"provider-b2\",id:\"{known_uuid}\",extra:\"ignored\"}}"),
        format!("{{source:'provider-b2',id:'{known_uuid}',extra:'ignored'}}"),
    ] {
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
